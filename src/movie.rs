use crate::source::{FrameSource, MovieInfo};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::mem::take;

/// Sent to the movie when a frame could not be read.
#[derive(EntityEvent, Debug, Clone)]
pub struct MovieError {
    #[event_target]
    pub movie: Entity,
    pub message: String,
}

#[derive(Component)]
pub struct Movie {
    source: Box<dyn FrameSource>,
    info: MovieInfo,
    image: Handle<Image>,
    position: f32,
    /// The frame that is in the image now.
    shown: Option<usize>,
    /// A frame that failed is not tried again until the position leaves it.
    failed: Option<usize>,
    scratch: Vec<u8>,
}

impl Movie {
    /// Create a placeholder image with the size as the movie.
    pub fn new(source: impl FrameSource, images: &mut Assets<Image>) -> Self {
        let info = source.info();
        let image = Image::new(
            Extent3d {
                width: info.size.x,
                height: info.size.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![0; (info.size.x * info.size.y * 4) as usize],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        Self {
            source: Box::new(source),
            info,
            image: images.add(image),
            position: 0.0,
            shown: None,
            failed: None,
            scratch: Vec::new(),
        }
    }

    pub fn image(&self) -> &Handle<Image> {
        &self.image
    }

    pub fn info(&self) -> MovieInfo {
        self.info
    }

    pub fn position(&self) -> f32 {
        self.position
    }

    /// Moves to `seconds`, kept between the start and the end of the movie.
    pub fn seek(&mut self, seconds: f32) {
        if seconds.is_finite() {
            self.position = seconds.clamp(0.0, self.info.duration());
        }
    }

    pub fn frame_at_position(&self) -> usize {
        self.info.frame_at(self.position)
    }

    pub fn shown_frame(&self) -> Option<usize> {
        self.shown
    }
}

pub struct MoviePlugin;
impl Plugin for MoviePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, show_frames);
    }
}

fn show_frames(
    mut commands: Commands,
    mut movies: Query<(Entity, &mut Movie)>,
    mut images: ResMut<Assets<Image>>,
) {
    for (entity, mut movie) in &mut movies {
        let wanted = movie.frame_at_position();
        if movie.failed.is_some_and(|failed| failed != wanted) {
            movie.failed = None;
        }
        if movie.shown == Some(wanted) || movie.failed == Some(wanted) {
            continue;
        }
        let movie = &mut *movie;
        match movie.source.read(wanted, &mut movie.scratch) {
            Ok(()) => {
                let Some(mut image) = images.get_mut(&movie.image) else {
                    continue;
                };
                image.data = Some(take(&mut movie.scratch));
                movie.shown = Some(wanted);
                movie.failed = None;
            }
            Err(error) => {
                movie.failed = Some(wanted);
                commands.trigger(MovieError {
                    movie: entity,
                    message: error.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceError;
    use std::sync::{Arc, Mutex};

    /// Frame `i` is filled with the byte `i`; frame 2 cannot be read.
    struct Fake {
        reads: Arc<Mutex<Vec<usize>>>,
    }
    impl FrameSource for Fake {
        fn info(&self) -> MovieInfo {
            MovieInfo {
                size: UVec2::new(1, 1),
                frames: 4,
                frame_rate: 1.0,
            }
        }
        fn read(&mut self, index: usize, out: &mut Vec<u8>) -> Result<(), SourceError> {
            self.reads.lock().unwrap().push(index);
            if index == 2 {
                return Err(SourceError::Empty("broken".into()));
            }
            *out = vec![index as u8; 4];
            Ok(())
        }
    }

    fn app() -> (App, Entity, Arc<Mutex<Vec<usize>>>) {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), MoviePlugin))
            .init_asset::<Image>();
        let movie = Movie::new(
            Fake {
                reads: reads.clone(),
            },
            &mut app.world_mut().resource_mut(),
        );
        let entity = app.world_mut().spawn(movie).id();
        (app, entity, reads)
    }

    fn pixel(app: &App, entity: Entity) -> u8 {
        let movie = app.world().get::<Movie>(entity).unwrap();
        let images = app.world().resource::<Assets<Image>>();
        images.get(movie.image()).unwrap().data.as_ref().unwrap()[0]
    }

    fn seek(app: &mut App, entity: Entity, t: f32) {
        app.world_mut().get_mut::<Movie>(entity).unwrap().seek(t);
        app.update();
    }

    #[test]
    fn the_frame_of_the_position_is_in_the_image() {
        let (mut app, entity, _) = app();
        app.update();
        assert_eq!(pixel(&app, entity), 0);
        seek(&mut app, entity, 3.5);
        assert_eq!(pixel(&app, entity), 3);
        seek(&mut app, entity, 1.0);
        assert_eq!(pixel(&app, entity), 1);
    }

    #[test]
    fn a_frame_that_is_already_shown_is_not_read_again() {
        let (mut app, entity, reads) = app();
        app.update();
        seek(&mut app, entity, 0.2);
        seek(&mut app, entity, 0.9);
        assert_eq!(*reads.lock().unwrap(), vec![0]);
    }

    #[test]
    fn a_frame_that_cannot_be_read_keeps_the_last_picture_and_is_not_tried_again() {
        let (mut app, entity, reads) = app();
        app.world_mut()
            .entity_mut(entity)
            .observe(|e: On<MovieError>, mut seen: Local<usize>| {
                *seen += 1;
                assert_eq!(*seen, 1, "reported once: {}", e.message);
            });
        seek(&mut app, entity, 1.0);
        seek(&mut app, entity, 2.0);
        seek(&mut app, entity, 2.5);
        assert_eq!(pixel(&app, entity), 1);
        let reads = reads.lock().unwrap();
        assert_eq!(reads.iter().filter(|i| **i == 2).count(), 1);
    }

    #[test]
    fn a_frame_that_failed_is_tried_again_after_the_position_has_left_it() {
        let (mut app, entity, reads) = app();
        seek(&mut app, entity, 1.0);
        seek(&mut app, entity, 2.0);
        // Back on the frame that is shown: nothing is read, but the failure is forgotten.
        seek(&mut app, entity, 1.0);
        seek(&mut app, entity, 2.0);
        let reads = reads.lock().unwrap();
        assert_eq!(reads.iter().filter(|i| **i == 2).count(), 2);
    }

    #[test]
    fn the_position_stays_between_the_start_and_the_end() {
        let (mut app, entity, _) = app();
        seek(&mut app, entity, -5.0);
        assert_eq!(app.world().get::<Movie>(entity).unwrap().position(), 0.0);
        seek(&mut app, entity, 99.0);
        let movie = app.world().get::<Movie>(entity).unwrap();
        assert_eq!(movie.position(), movie.info().duration());
        assert_eq!(movie.frame_at_position(), 3);
    }

    #[test]
    fn a_position_that_is_not_a_number_is_ignored() {
        let (mut app, entity, _) = app();
        seek(&mut app, entity, 1.0);
        seek(&mut app, entity, f32::NAN);
        assert_eq!(app.world().get::<Movie>(entity).unwrap().position(), 1.0);
    }
}
