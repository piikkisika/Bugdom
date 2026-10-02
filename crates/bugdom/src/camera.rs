//! The game camera's lens, fog and clear colour, and a debug fly camera.

use std::f32::consts::FRAC_PI_2;

use bevy::camera::{ClearColorConfig, Exposure};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};

use crate::level::{CAMERA_FOV, CurrentLevel, HITHER_DISTANCE};
use crate::state::AppState;
use crate::terrain::{PlayerStart, TerrainMap, TerrainSystems};

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(
                OnEnter(AppState::InGame),
                (
                    set_up_level_view,
                    place_fly_camera.after(TerrainSystems::Spawn),
                ),
            )
            .add_systems(
                Update,
                (grab_cursor, fly_camera_look, fly_camera_move).chain(),
            );
    }
}

/// The camera that shows the game world.
#[derive(Component, Debug, Default)]
pub struct GameCamera;

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        GameCamera,
        Camera3d::default(),
        // The original has no tone mapping; its colours go straight to the screen.
        Tonemapping::None,
        // Exposure 1, so that light values are not scaled.
        Exposure {
            ev100: -(1.2f32.log2()),
        },
    ));
}

/// Sets the lens, fog and clear colour for the current level.
///
/// Port of the view and fog set-up in `InitArea` (original/src/System/Main.c).
/// The original's fog is plane-based linear fog; Bevy's linear fog measures
/// the distance from the camera, which differs slightly at the screen's edges.
fn set_up_level_view(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    mut cameras: Query<(Entity, &mut Camera, &mut Projection), With<GameCamera>>,
) {
    let settings = level.def().settings();
    let yon = settings.yon();
    let [r, g, b] = settings.fog_color;
    let [cr, cg, cb] = settings.clear_color();
    for (entity, mut camera, mut projection) in &mut cameras {
        *projection = Projection::Perspective(PerspectiveProjection {
            fov: CAMERA_FOV,
            near: HITHER_DISTANCE,
            far: yon,
            ..default()
        });
        camera.clear_color = ClearColorConfig::Custom(Color::srgb(cr, cg, cb));
        commands.entity(entity).insert(DistanceFog {
            color: Color::srgb(r, g, b),
            falloff: FogFalloff::Linear {
                start: settings.fog_start * yon,
                end: settings.fog_end * yon,
            },
            ..default()
        });
    }
}

/// Until the player exists, the camera flies freely from above the player's
/// start, looking the way the player would.
pub fn place_fly_camera(
    mut commands: Commands,
    start: Res<PlayerStart>,
    map: Res<TerrainMap>,
    cameras: Query<Entity, With<GameCamera>>,
) {
    let ground = map.floor_height(start.position.x, start.position.y);
    info!(
        "Player start: {} at height {ground}, aim {}",
        start.position, start.aim
    );
    let camera = FlyCamera::new(start.yaw(), -0.35);
    let transform =
        Transform::from_translation(start.position.extend(0.0).xzy().with_y(ground + 400.0))
            .with_rotation(camera.rotation());
    for entity in &cameras {
        commands.entity(entity).insert((camera.clone(), transform));
    }
}

/// Debug free-flying camera, modelled on Bevy's `bevy_camera_controller` free
/// camera. Hold the right mouse button to look around; WASD to move, E and Q
/// up and down, Shift to go faster, the mouse wheel to change speed.
#[derive(Component, Debug, Clone)]
pub struct FlyCamera {
    /// Speed in world units per second.
    pub speed: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl FlyCamera {
    /// Radians per pixel of mouse movement.
    const SENSITIVITY: f32 = 0.003;
    const RUN_MULTIPLIER: f32 = 4.0;
    /// Speed factor per line of mouse-wheel scrolling.
    const SCROLL_FACTOR: f32 = 1.2;

    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            speed: 800.0,
            yaw,
            pitch,
        }
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }
}

fn grab_cursor(
    buttons: Res<ButtonInput<MouseButton>>,
    cameras: Query<(), With<FlyCamera>>,
    mut cursors: Query<&mut CursorOptions>,
) {
    if cameras.is_empty() {
        return;
    }
    let grab = if buttons.just_pressed(MouseButton::Right) {
        true
    } else if buttons.just_released(MouseButton::Right) {
        false
    } else {
        return;
    };
    for mut cursor in &mut cursors {
        cursor.grab_mode = if grab {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        cursor.visible = !grab;
    }
}

fn fly_camera_look(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cameras: Query<(&mut Transform, &mut FlyCamera)>,
) {
    for (mut transform, mut camera) in &mut cameras {
        if buttons.pressed(MouseButton::Right) {
            camera.yaw -= motion.delta.x * FlyCamera::SENSITIVITY;
            camera.pitch = (camera.pitch - motion.delta.y * FlyCamera::SENSITIVITY)
                .clamp(-FRAC_PI_2, FRAC_PI_2);
        }
        transform.rotation = camera.rotation();
    }
}

fn fly_camera_move(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut cameras: Query<(&mut Transform, &mut FlyCamera)>,
) {
    let axis = |positive, negative| {
        f32::from(u8::from(keys.pressed(positive))) - f32::from(u8::from(keys.pressed(negative)))
    };
    let input = Vec3::new(
        axis(KeyCode::KeyD, KeyCode::KeyA),
        axis(KeyCode::KeyE, KeyCode::KeyQ),
        axis(KeyCode::KeyW, KeyCode::KeyS),
    );
    for (mut transform, mut camera) in &mut cameras {
        if scroll.delta.y != 0.0 {
            camera.speed *= FlyCamera::SCROLL_FACTOR.powf(scroll.delta.y.signum());
        }
        if input == Vec3::ZERO {
            continue;
        }
        let run = if keys.pressed(KeyCode::ShiftLeft) {
            FlyCamera::RUN_MULTIPLIER
        } else {
            1.0
        };
        let direction =
            transform.right() * input.x + Vec3::Y * input.y + transform.forward() * input.z;
        transform.translation +=
            direction.normalize_or_zero() * camera.speed * run * time.delta_secs();
    }
}
