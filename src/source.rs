//! Where the frames come from.
//!
//! A source knows how many frames there are and how to read one. How the frames are stored is the business of the source,
//! so the player does not care and a backend can be replaced without touching the rest.

use bevy::math::UVec2;
use image::{image_dimensions, open as img_open};
use std::{
    fs::read_dir,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("{0}: no frames (jpg, jpeg, png, webp) were found")]
    Empty(PathBuf),
    #[error("{path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("frame {index} is {got:?} but the movie is {want:?}")]
    SizeMismatch {
        index: usize,
        got: UVec2,
        want: UVec2,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovieInfo {
    pub size: UVec2,
    pub frames: usize,
    pub frame_rate: f32,
}

impl MovieInfo {
    pub fn duration(&self) -> f32 {
        self.frames as f32 / self.frame_rate
    }

    /// The frame to show at `t` seconds. If out of range, returns the first or the last.
    pub fn frame_at(&self, t: f32) -> usize {
        let index = (t.max(0.0) * self.frame_rate).floor() as usize;
        index.min(self.frames.saturating_sub(1))
    }
}

pub trait FrameSource: Send + Sync + 'static {
    fn info(&self) -> MovieInfo;

    /// Writes frame at `index` into `out` as RGBA, 8 bits a channel, `size.x * size.y * 4` bytes.
    fn read(&mut self, index: usize, out: &mut Vec<u8>) -> Result<(), SourceError>;
}

/// A folder of images, one for each frame.
pub struct ImageSequence {
    files: Vec<PathBuf>,
    info: MovieInfo,
}

impl ImageSequence {
    /// Opens the folder. The size of the first image is the size of the movie.
    pub fn open(dir: impl AsRef<Path>, frame_rate: f32) -> Result<Self, SourceError> {
        let dir = dir.as_ref();
        let read_dir = read_dir(dir).map_err(|e| SourceError::Read {
            path: dir.into(),
            message: e.to_string(),
        })?;
        let mut files: Vec<PathBuf> = read_dir
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| {
                path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "jpg" | "jpeg" | "png" | "webp"
                    )
                })
            })
            .collect();
        files.sort();
        let first = files
            .first()
            .ok_or_else(|| SourceError::Empty(dir.into()))?;
        let (width, height) = image_dimensions(first).map_err(|e| SourceError::Read {
            path: first.clone(),
            message: e.to_string(),
        })?;
        let info = MovieInfo {
            size: UVec2::new(width, height),
            frames: files.len(),
            frame_rate,
        };
        Ok(Self { files, info })
    }
}

impl FrameSource for ImageSequence {
    fn info(&self) -> MovieInfo {
        self.info
    }

    fn read(&mut self, index: usize, out: &mut Vec<u8>) -> Result<(), SourceError> {
        let path = &self.files[index];
        let rgba = img_open(path)
            .map_err(|e| SourceError::Read {
                path: path.clone(),
                message: e.to_string(),
            })?
            .into_rgba8();
        let got = UVec2::new(rgba.width(), rgba.height());
        if got != self.info.size {
            return Err(SourceError::SizeMismatch {
                index,
                got,
                want: self.info.size,
            });
        }
        *out = rgba.into_raw();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::fs::write;
    use tempfile::tempdir;

    fn info(frames: usize, frame_rate: f32) -> MovieInfo {
        MovieInfo {
            size: UVec2::new(2, 2),
            frames,
            frame_rate,
        }
    }

    #[test]
    fn the_frame_is_the_last_one_that_has_begun() {
        let info = info(10, 10.0);
        assert_eq!(info.frame_at(0.0), 0);
        assert_eq!(info.frame_at(0.099), 0);
        assert_eq!(info.frame_at(0.1), 1);
        assert_eq!(info.frame_at(0.55), 5);
    }

    #[test]
    fn before_the_start_and_after_the_end_the_frame_is_the_first_and_the_last() {
        let info = info(10, 10.0);
        assert_eq!(info.frame_at(-3.0), 0);
        assert_eq!(info.frame_at(1.0), 9);
        assert_eq!(info.frame_at(99.0), 9);
        assert_eq!(info.duration(), 1.0);
    }

    #[test]
    fn a_folder_without_frames_is_an_error() {
        let dir = tempdir().unwrap();
        write(dir.path().join("notes.txt"), "x").unwrap();
        assert!(matches!(
            ImageSequence::open(dir.path(), 10.0),
            Err(SourceError::Empty(_))
        ));
    }

    #[test]
    fn frames_are_read_in_the_order_of_their_names_and_must_agree_in_size() {
        let dir = tempdir().unwrap();
        let save = |name: &str, w: u32, h: u32, rgb: [u8; 3]| {
            RgbImage::from_pixel(w, h, Rgb(rgb))
                .save(dir.path().join(name))
                .unwrap();
        };
        save("0002.png", 4, 2, [0, 0, 255]);
        save("0001.png", 4, 2, [255, 0, 0]);
        save("0003.png", 2, 2, [0, 255, 0]);
        let mut seq = ImageSequence::open(dir.path(), 10.0).unwrap();
        assert_eq!(seq.info().size, UVec2::new(4, 2));
        assert_eq!(seq.info().frames, 3);

        let mut out = Vec::new();
        seq.read(0, &mut out).unwrap();
        assert_eq!(&out[..4], &[255, 0, 0, 255]);
        seq.read(1, &mut out).unwrap();
        assert_eq!(&out[..4], &[0, 0, 255, 255]);
        assert!(matches!(
            seq.read(2, &mut out),
            Err(SourceError::SizeMismatch { index: 2, .. })
        ));
    }
}
