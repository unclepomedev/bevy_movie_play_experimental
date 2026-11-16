//! Plays a movie in Bevy: the frame for a position goes into an `Image`, which anything can show.
//! The movie does not run by itself. Whoever owns the time says where the movie is [`Movie::seek`], and the frame for that moment is shown.
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_movie_play_experimental::*;
//! fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
//!     let source = ImageSequence::open("frames/", 10.0).unwrap();
//!     let movie = Movie::new(source, &mut images);
//!     commands.spawn((ImageNode::new(movie.image().clone()), movie));
//! }
//! ```

mod movie;
mod source;

pub use movie::{Movie, MovieError, MoviePlugin};
pub use source::{FrameSource, ImageSequence, MovieInfo, SourceError};
