//! Plays a folder of frames. Without an argument it makes a short test movie in the temp folder.
//!
//! Execution: cargo run --example sequence_player -- sandbox/frames/ [frame_rate]
//!
//! What to verify:
//! - The picture is the movie, and it plays once through and stops at the last frame.
//! - Space pauses and resumes. Left/Right move a second. `,` and `.` move a frame. R starts again.
//! - The test movie is a bar that crosses the picture, with the frame number written as the height of a
//!   column on the left, so a wrong frame is easy to see.

use bevy::prelude::*;
use bevy_movie_play_experimental::{ImageSequence, Movie, MoviePlugin};
use image::{Rgb, RgbImage};
use std::{env, fs::create_dir_all, path::PathBuf};

#[derive(Component)]
struct Playing(bool);

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, MoviePlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, (advance, keys))
        .run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut args = env::args().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(make_test_movie);
    let frame_rate = args.next().and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let source = ImageSequence::open(&dir, frame_rate).expect("cannot open the folder");
    let movie = Movie::new(source, &mut images);
    commands.spawn(Camera2d);
    commands.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        children![(
            Node {
                width: px(960),
                aspect_ratio: Some(16.0 / 9.0),
                ..default()
            },
            ImageNode::new(movie.image().clone()),
            movie,
            Playing(true),
        )],
    ));
}

fn advance(time: Res<Time>, mut movies: Query<(&mut Movie, &Playing)>) {
    for (mut movie, playing) in &mut movies {
        if playing.0 {
            let end = movie.info().duration();
            let at = movie.position() + time.delta_secs();
            movie.seek(at.min(end));
        }
    }
}

fn keys(input: Res<ButtonInput<KeyCode>>, mut movies: Query<(&mut Movie, &mut Playing)>) {
    for (mut movie, mut playing) in &mut movies {
        let frame = 1.0 / movie.info().frame_rate;
        let at = movie.position();
        if input.just_pressed(KeyCode::Space) {
            playing.0 = !playing.0;
        }
        if input.just_pressed(KeyCode::ArrowLeft) {
            movie.seek((at - 1.0).max(0.0));
        }
        if input.just_pressed(KeyCode::ArrowRight) {
            movie.seek(at + 1.0);
        }
        if input.just_pressed(KeyCode::Comma) {
            playing.0 = false;
            movie.seek((at - frame).max(0.0));
        }
        if input.just_pressed(KeyCode::Period) {
            playing.0 = false;
            movie.seek(at + frame);
        }
        if input.just_pressed(KeyCode::KeyR) {
            movie.seek(0.0);
            playing.0 = true;
        }
    }
}

/// 60 frames of 320x180: a bar goes across, and a column on the left grows with the frame number.
fn make_test_movie() -> PathBuf {
    let dir = env::temp_dir().join("bevy_movie_play_experimental_test_movie");
    create_dir_all(&dir).unwrap();
    for frame in 0..60u32 {
        let mut img = RgbImage::from_pixel(320, 180, Rgb([12, 24, 32]));
        let x0 = 20 + frame * 4;
        for y in 70..110 {
            for x in x0..(x0 + 30).min(320) {
                img.put_pixel(x, y, Rgb([230, 240, 255]));
            }
        }
        for y in (180 - frame * 3)..180 {
            for x in 0..12 {
                img.put_pixel(x, y, Rgb([240, 180, 60]));
            }
        }
        img.save(dir.join(format!("{frame:04}.png"))).unwrap();
    }
    dir
}
