//! Development aids.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::camera::{CameraSystems, FlyCamera, GameCamera, fly};
use crate::state::AppState;

/// Environment variable naming a PNG file: when set, the game takes a
/// screenshot shortly after the level starts, saves it there and exits.
/// Used for headless rendering checks (see CLAUDE.md).
pub const CAPTURE_ENV: &str = "BUGDOM_CAPTURE";

/// Environment variable that places the debug camera for a capture:
/// `x,y,z,yaw,pitch`, in world units and radians.
pub const CAMERA_ENV: &str = "BUGDOM_CAMERA";

/// Real time to wait after the level starts, so that shaders and textures
/// are ready before the screenshot.
const CAPTURE_DELAY_SECS: f32 = 2.0;
/// Frames to wait after requesting the screenshot, which is saved
/// asynchronously.
const FRAMES_TO_SAVE: u32 = 30;

pub struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        if let Ok(path) = std::env::var(CAPTURE_ENV) {
            app.insert_resource(Capture {
                path,
                delay: CAPTURE_DELAY_SECS,
                frames_since_taken: None,
            })
            .add_systems(Update, capture.run_if(in_state(AppState::InGame)));
        }
        if let Ok(value) = std::env::var(CAMERA_ENV) {
            match parse_camera(&value) {
                Some(pose) => {
                    app.insert_resource(pose).add_systems(
                        OnEnter(AppState::InGame),
                        override_camera.after(CameraSystems::Place),
                    );
                }
                None => warn!("{CAMERA_ENV}={value} is not x,y,z,yaw,pitch"),
            }
        }
    }
}

#[derive(Resource, Debug)]
struct Capture {
    path: String,
    delay: f32,
    frames_since_taken: Option<u32>,
}

fn capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut capture: ResMut<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    match &mut capture.frames_since_taken {
        None => {
            capture.delay -= time.delta_secs();
            if capture.delay <= 0.0 {
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(capture.path.clone()));
                capture.frames_since_taken = Some(0);
            }
        }
        Some(frames) => {
            *frames += 1;
            if *frames > FRAMES_TO_SAVE {
                exit.write(AppExit::Success);
            }
        }
    }
}

/// A camera placement from [`CAMERA_ENV`].
#[derive(Resource, Debug, Clone, Copy)]
struct CameraPose {
    position: Vec3,
    yaw: f32,
    pitch: f32,
}

fn parse_camera(value: &str) -> Option<CameraPose> {
    let numbers: Vec<f32> = value
        .split(',')
        .map(|n| n.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, z, yaw, pitch] = numbers.try_into().ok()?;
    Some(CameraPose {
        position: Vec3::new(x, y, z),
        yaw,
        pitch,
    })
}

fn override_camera(
    mut commands: Commands,
    pose: Res<CameraPose>,
    cameras: Query<Entity, With<GameCamera>>,
) {
    let camera = FlyCamera::new(pose.yaw, pose.pitch);
    let transform = Transform::from_translation(pose.position).with_rotation(camera.rotation());
    for entity in &cameras {
        fly(&mut commands, entity, camera.clone(), transform);
    }
}
