//! A source that reads the H.264 video of an MP4 file. It is behind the feature `mp4`.
//!
//! The demuxer is `mp4` and the decoder is `openh264` (built from its source, no system library needed).
//!
//! The video has to be encoded without B-frames (`ffmpeg ... -c:v libx264 -bf 0`), because a decoder that gives
//! every picture at once cannot reorder them. A short interval of keyframes (`-g 10`) keeps seeking cheap: to
//! show a frame, decoding goes back to the keyframe before it and runs forward. Frames are counted by the
//! samples of the file, so the frame rate has to be constant.

use crate::source::{FrameSource, MovieInfo, SourceError};
use bevy::math::UVec2;
use mp4::{MediaType, Mp4Reader, Mp4Track};
use openh264::{decoder::Decoder, formats::YUVSource};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

const START_CODE: [u8; 4] = [0, 0, 0, 1];

fn failure(path: &Path, message: String) -> SourceError {
    SourceError::Read {
        path: path.into(),
        message,
    }
}

pub struct Mp4Source {
    path: PathBuf,
    reader: Mp4Reader<BufReader<File>>,
    track_id: u32,
    info: MovieInfo,
    /// The numbers of the samples that are keyframes, counted from 1, in order.
    keyframes: Vec<u32>,
    /// The parameter sets of the stream, in the form that the decoder reads.
    parameter_sets: Vec<u8>,
    /// How many bytes tell the length of a NAL unit in a sample.
    length_size: usize,
    decoder: Decoder,
    /// The number of the sample that the decoder expects next, if it is in a state to take it.
    next: Option<u32>,
    packet: Vec<u8>,
}

impl Mp4Source {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let path = path.as_ref();
        let fail = |message: String| failure(path, message);
        let reader = read_header(path).map_err(fail)?;
        let track = h264_track(&reader).map_err(fail)?;
        let info = movie_info(track).map_err(fail)?;
        let keyframes = keyframes(track).map_err(fail)?;
        let (parameter_sets, length_size) = avc_config(track).map_err(fail)?;
        let track_id = track.track_id();
        Ok(Self {
            path: path.into(),
            track_id,
            info,
            keyframes,
            parameter_sets,
            length_size,
            decoder: Decoder::new().map_err(|e| fail(e.to_string()))?,
            reader,
            next: None,
            packet: Vec::new(),
        })
    }

    /// Decodes the samples from `start` to `target`, and writes the picture of the last one into `out`.
    fn decode(&mut self, start: u32, target: u32, out: &mut Vec<u8>) -> Result<(), SourceError> {
        for id in start..=target {
            let sample = self
                .reader
                .read_sample(self.track_id, id)
                .map_err(|e| failure(&self.path, e.to_string()))?
                .ok_or_else(|| failure(&self.path, format!("sample {id} is missing")))?;
            self.packet.clear();
            if sample.is_sync {
                self.packet.extend(&self.parameter_sets);
            }
            if !to_annex_b(&sample.bytes, self.length_size, &mut self.packet) {
                return Err(failure(&self.path, format!("sample {id} is cut short")));
            }
            let picture = self
                .decoder
                .decode(&self.packet)
                .map_err(|e| failure(&self.path, format!("sample {id}: {e}")))?;
            if id == target {
                let picture = picture
                    .ok_or_else(|| failure(&self.path, format!("sample {id} gave no picture")))?;
                let (w, h) = picture.dimensions();
                if UVec2::new(w as u32, h as u32) != self.info.size {
                    return Err(failure(
                        &self.path,
                        format!("sample {id} is {w}x{h}, not {:?}", self.info.size),
                    ));
                }
                out.resize(w * h * 4, 0);
                picture.write_rgba8(out);
            }
        }
        Ok(())
    }
}

impl FrameSource for Mp4Source {
    fn info(&self) -> MovieInfo {
        self.info
    }

    fn read(&mut self, index: usize, out: &mut Vec<u8>) -> Result<(), SourceError> {
        let target = index as u32 + 1;
        let key = self.keyframes[self.keyframes.partition_point(|k| *k <= target) - 1];
        // The decoder goes on where it is when the frame is ahead of it in the same run of frames.
        let start = match self.next {
            Some(next) if (key..=target).contains(&next) => next,
            _ => key,
        };
        self.next = None;
        self.decode(start, target, out)?;
        self.next = Some(target + 1);
        Ok(())
    }
}

fn read_header(path: &Path) -> Result<Mp4Reader<BufReader<File>>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    Mp4Reader::read_header(BufReader::new(file), size).map_err(|e| e.to_string())
}

/// The H.264 track. Its frames have to be stored in the order of display.
fn h264_track(reader: &Mp4Reader<BufReader<File>>) -> Result<&Mp4Track, String> {
    let track = reader
        .tracks()
        .values()
        .find(|t| t.media_type().is_ok_and(|m| m == MediaType::H264))
        .ok_or("no H.264 video track")?;
    if track.trak.mdia.minf.stbl.ctts.is_some() {
        return Err("the frames are reordered (B-frames): encode with `-bf 0`".into());
    }
    Ok(track)
}

fn movie_info(track: &Mp4Track) -> Result<MovieInfo, String> {
    let frames = track.sample_count();
    // `Mp4Track::frame_rate` divides whole numbers, which turns 29.97 into 29.
    let seconds = track.trak.mdia.mdhd.duration as f64 / f64::from(track.timescale());
    let frame_rate = (f64::from(frames) / seconds) as f32;
    if frames == 0 || !(frame_rate.is_finite() && frame_rate > 0.0) {
        return Err(format!("{frames} frames at {frame_rate} frames a second"));
    }
    Ok(MovieInfo {
        size: UVec2::new(track.width().into(), track.height().into()),
        frames: frames as usize,
        frame_rate,
    })
}

/// The numbers of the samples that are keyframes, counted from 1.
fn keyframes(track: &Mp4Track) -> Result<Vec<u32>, String> {
    let keyframes = match &track.trak.mdia.minf.stbl.stss {
        Some(stss) => stss.entries.clone(),
        None => (1..=track.sample_count()).collect(),
    };
    if keyframes.first() != Some(&1) {
        return Err("the first frame is not a keyframe".into());
    }
    Ok(keyframes)
}

/// The parameter sets of the stream in the form that the decoder reads, and how many bytes tell the length of
/// a NAL unit in a sample.
fn avc_config(track: &Mp4Track) -> Result<(Vec<u8>, usize), String> {
    let avcc = &track
        .trak
        .mdia
        .minf
        .stbl
        .stsd
        .avc1
        .as_ref()
        .ok_or("the track has no AVC configuration")?
        .avcc;
    let mut parameter_sets = Vec::new();
    for nal in avcc
        .sequence_parameter_sets
        .iter()
        .chain(&avcc.picture_parameter_sets)
    {
        parameter_sets.extend(START_CODE);
        parameter_sets.extend(&nal.bytes);
    }
    Ok((parameter_sets, usize::from(avcc.length_size_minus_one) + 1))
}

/// Turns the NAL units of a sample, each with its length in front, into the units with a start code in front.
/// It is `false` when the sample ends in the middle of a unit.
fn to_annex_b(mut sample: &[u8], length_size: usize, out: &mut Vec<u8>) -> bool {
    while !sample.is_empty() {
        if sample.len() < length_size {
            return false;
        }
        let (length, rest) = sample.split_at(length_size);
        let length = length
            .iter()
            .fold(0usize, |n, byte| n << 8 | usize::from(*byte));
        if rest.len() < length {
            return false;
        }
        let (nal, rest) = rest.split_at(length);
        out.extend(START_CODE);
        out.extend(nal);
        sample = rest;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAMP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ramp.mp4");
    const RAMP_29_97: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ramp_29_97.mp4");
    const RAMP_WITH_B_FRAMES: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ramp_bframes.mp4");

    #[test]
    fn units_with_their_length_in_front_get_a_start_code_instead() {
        let mut out = Vec::new();
        assert!(to_annex_b(&[0, 0, 0, 2, 7, 8, 0, 0, 0, 1, 9], 4, &mut out));
        assert_eq!(out, [0, 0, 0, 1, 7, 8, 0, 0, 0, 1, 9]);
        out.clear();
        assert!(to_annex_b(&[0, 1, 5], 2, &mut out));
        assert_eq!(out, [0, 0, 0, 1, 5]);
    }

    #[test]
    fn a_sample_that_is_cut_short_is_refused() {
        assert!(!to_annex_b(&[0, 0, 0, 9, 1], 4, &mut Vec::new()));
        assert!(!to_annex_b(&[0, 0], 4, &mut Vec::new()));
    }

    #[test]
    fn the_file_says_what_it_is() {
        let info = Mp4Source::open(RAMP).unwrap().info();
        assert_eq!(info.size, UVec2::new(64, 64));
        assert_eq!(info.frames, 30);
        assert_eq!(info.frame_rate, 10.0);
    }

    #[test]
    fn a_frame_rate_with_a_fraction_is_kept() {
        let info = Mp4Source::open(RAMP_29_97).unwrap().info();
        assert_eq!(info.frames, 30);
        assert!(
            (info.frame_rate - 30000.0 / 1001.0).abs() < 0.001,
            "{}",
            info.frame_rate
        );
    }

    #[test]
    fn a_file_with_b_frames_is_refused_with_the_way_out() {
        let Err(SourceError::Read { message, .. }) = Mp4Source::open(RAMP_WITH_B_FRAMES) else {
            panic!("it must be refused");
        };
        assert!(message.contains("-bf 0"), "{message}");
    }

    #[test]
    fn a_file_that_is_not_there_is_an_error() {
        assert!(matches!(
            Mp4Source::open("no/such/file.mp4"),
            Err(SourceError::Read { .. })
        ));
    }

    /// The frames of the file are flat grays, and the gray goes up by a step with each frame, so the first
    /// byte of a picture says which frame it is.
    fn gray(source: &mut Mp4Source, index: usize) -> u8 {
        let mut out = Vec::new();
        source.read(index, &mut out).unwrap();
        assert_eq!(out.len(), 64 * 64 * 4);
        out[0]
    }

    #[test]
    fn the_frames_in_order_are_all_different_and_rise() {
        let mut source = Mp4Source::open(RAMP).unwrap();
        let grays: Vec<u8> = (0..30).map(|i| gray(&mut source, i)).collect();
        assert!(
            grays.windows(2).all(|pair| pair[1] >= pair[0] + 4),
            "{grays:?}"
        );
    }

    #[test]
    fn a_frame_is_the_same_wherever_the_reading_comes_from() {
        let mut source = Mp4Source::open(RAMP).unwrap();
        let in_order: Vec<u8> = (0..30).map(|i| gray(&mut source, i)).collect();
        // Back to the start, back in the same run, on to the next run, to the same keyframe, to the last.
        for index in [25, 3, 4, 29, 0, 12, 11, 10, 9, 21, 20, 5, 6, 7] {
            let got = gray(&mut source, index);
            assert!(
                got.abs_diff(in_order[index]) <= 1,
                "frame {index}: {got} but {} when in order",
                in_order[index]
            );
        }
    }

    #[test]
    fn it_plays_through_a_movie() {
        use crate::Movie;
        use bevy::prelude::*;
        let mut images = Assets::<Image>::default();
        let mut movie = Movie::new(Mp4Source::open(RAMP).unwrap(), &mut images);
        movie.seek(1.55);
        assert_eq!(movie.frame_at_position(), 15);
    }
}
