//! The game camera: its lens, fog and lights, the camera that follows the
//! player, and a debug fly camera (F1 toggles it).

use std::f32::consts::{FRAC_PI_2, PI};

use avian3d::prelude::TransformInterpolation;
use bevy::camera::{ClearColorConfig, Exposure};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::light::GlobalAmbientLight;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};

use crate::input::{Action, ControlInput, InputEnabled};
use crate::level::{
    AMBIENT_BRIGHTNESS, CAMERA_FOV, CurrentLevel, FILL_BRIGHTNESS, HITHER_DISTANCE,
};
use crate::math::{quick_distance, yaw_from_point_to_point};
use crate::player::{Player, PlayerSystems, PlayerToCameraAngle};
use crate::state::AppState;
use crate::terrain::{LayerKind, SUPERTILE_TILES, TILE_SIZE, TerrainMap};

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(
                OnEnter(AppState::InGame),
                (
                    set_up_level_view,
                    spawn_level_lights,
                    place_follow_camera
                        .in_set(CameraSystems::Place)
                        .after(PlayerSystems::Spawn),
                ),
            )
            .add_systems(
                FixedUpdate,
                follow_player
                    .after(PlayerSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                Update,
                (
                    toggle_fly_camera,
                    (grab_cursor, fly_camera_look, fly_camera_move).chain(),
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CameraSystems {
    /// Places the camera behind the player when the level starts.
    Place,
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

/// The level's lights for objects: the ambient light and the two fill
/// lights, with the level type's colours (`InitArea` in
/// original/src/System/Main.c). The terrain is prelit and ignores them.
///
/// Bevy's diffuse term divides by π, so each light's illuminance is
/// multiplied by π to give the original's `colour × brightness × N·L`.
/// Objects get a material that matches the original's lighting exactly
/// with the collision framework; until then they are close.
fn spawn_level_lights(mut commands: Commands, level: Res<CurrentLevel>) {
    let settings = level.def().settings();
    let [ambient, fill0, fill1] = settings.light_colors;
    let color = |[r, g, b]: [f32; 3]| Color::srgb(r, g, b);
    commands.insert_resource(GlobalAmbientLight {
        color: color(ambient),
        brightness: AMBIENT_BRIGHTNESS * AMBIENT_TO_BEVY,
        ..default()
    });
    for ((fill, brightness), direction) in [fill0, fill1]
        .into_iter()
        .zip(FILL_BRIGHTNESS)
        .zip(settings.fill_directions())
    {
        commands.spawn((
            Name::new("Fill light"),
            DirectionalLight {
                color: color(fill),
                illuminance: brightness * PI,
                ..default()
            },
            Transform::default().looking_to(direction, Vec3::X),
            DespawnOnExit(AppState::InGame),
        ));
    }
}

/// Converts the original's ambient brightness to Bevy's ambient brightness.
const AMBIENT_TO_BEVY: f32 = 1.0;

/// The camera that follows the player, swinging round behind it lazily.
///
/// Port of `MoveCamera_Manual` and `UpdateCamera`
/// (original/src/QD3D/Camera.c). Its state is the camera's position and the
/// point it looks at (`currentCameraCoords` and `currentCameraLookAt`).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct FollowCamera {
    pub from: Vec3,
    pub to: Vec3,
    /// How far behind the player the camera tries to stay
    /// (`gCameraDistFromMe`); the zoom keys change it.
    pub distance: f32,
}

impl FollowCamera {
    /// How quickly the look-at point catches up with the player, per second
    /// (`gCameraLookAtAccel`); vertically it is 70% of this.
    const LOOK_AT_ACCEL: f32 = 9.0;
    const LOOK_AT_ACCEL_Y_FACTOR: f32 = 0.7;
    /// How quickly the camera catches up horizontally and vertically, per
    /// second (`gCameraFromAccel`, `gCameraFromAccelY`).
    const FROM_ACCEL: f32 = 4.5;
    const FROM_ACCEL_Y: f32 = 1.5;
    /// How much higher the camera sits per unit of distance
    /// (`gCameraHeightFactor`).
    const HEIGHT_FACTOR: f32 = 0.3;
    /// Height of the look-at point above the player's feet
    /// (`gCameraLookAtYOff`).
    const LOOK_AT_HEIGHT: f32 = 95.0;
    const START_DISTANCE: f32 = 500.0;
    /// Zoom limits and speed (`CAMERA_CLOSEST`, `CAMERA_FARTHEST`), in
    /// units and units per second.
    const CLOSEST: f32 = 150.0;
    const FARTHEST: f32 = 800.0;
    const ZOOM_SPEED: f32 = 200.0;
    /// The camera stays at least this far above the look-at point
    /// (`CAM_MINY`).
    const MIN_HEIGHT: f32 = 20.0;
    /// The largest horizontal gap that pulls the look-at point and the
    /// camera along; a bigger gap pulls no harder.
    const LOOK_AT_MAX_GAP: f32 = 350.0;
    const FROM_MAX_GAP: f32 = 500.0;
    /// The furthest the camera moves horizontally in one update: a
    /// supertile, which kept the original's terrain scrolling safe.
    const MAX_STEP: f32 = SUPERTILE_TILES as f32 * TILE_SIZE;
    /// Clearance from the floor and ceiling.
    const TERRAIN_CLEARANCE: f32 = 60.0;
    /// `InitCamera` primes the camera with this many updates of this
    /// length.
    const PRIME_UPDATES: usize = 100;
    const PRIME_DT: f32 = 1.0 / 20.0;

    /// A camera just behind and above the player. Port of the start of
    /// `InitCamera`.
    pub fn behind(player: Vec3, yaw: f32) -> Self {
        Self {
            from: player + Vec3::new(yaw.sin() * 10.0, 300.0, yaw.cos() * 10.0),
            to: player + Vec3::Y * 100.0,
            distance: Self::START_DISTANCE,
        }
    }

    /// Zooms with the zoom keys. Port of `UpdateCamera`.
    fn zoom(&mut self, input: &ControlInput, dt: f32) {
        if input.held(Action::ZoomIn) {
            self.distance = (self.distance - Self::ZOOM_SPEED * dt).max(Self::CLOSEST);
        } else if input.held(Action::ZoomOut) {
            self.distance = (self.distance + Self::ZOOM_SPEED * dt).min(Self::FARTHEST);
        }
    }

    /// Moves the camera toward its place behind the player at `feet`, and
    /// updates the angle of the camera around the player.
    ///
    /// Port of `MoveCamera_Manual`. Lifting the camera over objects that
    /// block it (`CTYPE_BLOCKCAMERA`) arrives with the collision framework.
    pub fn update(
        &mut self,
        feet: Vec3,
        swivel: f32,
        map: &TerrainMap,
        camera_angle: &mut f32,
        dt: f32,
    ) {
        *camera_angle = PI - yaw_from_point_to_point(PI - *camera_angle, feet.xz(), self.from.xz());

        // The look-at point chases a point above the player.
        let target = feet + Vec3::Y * Self::LOOK_AT_HEIGHT;
        let gap = target - self.to;
        let gap_xz = gap.xz().clamp(
            Vec2::splat(-Self::LOOK_AT_MAX_GAP),
            Vec2::splat(Self::LOOK_AT_MAX_GAP),
        );
        let to = self.to
            + Vec3::new(
                gap_xz.x * (dt * Self::LOOK_AT_ACCEL),
                gap.y * (dt * Self::LOOK_AT_ACCEL * Self::LOOK_AT_ACCEL_Y_FACTOR),
                gap_xz.y * (dt * Self::LOOK_AT_ACCEL),
            );

        // The camera chases the point at its distance from the player, in
        // the direction it already is.
        let player_to_camera = (self.from.xz() - feet.xz()).normalize_or_zero();
        let target_xz = feet.xz() + player_to_camera * self.distance;
        let step = ((target_xz - self.from.xz()).clamp(
            Vec2::splat(-Self::FROM_MAX_GAP),
            Vec2::splat(Self::FROM_MAX_GAP),
        ) * (dt * Self::FROM_ACCEL))
            .clamp(Vec2::splat(-Self::MAX_STEP), Vec2::splat(Self::MAX_STEP));
        let mut from = self.from + Vec3::new(step.x, 0.0, step.y);

        if swivel != 0.0 {
            from = to + Quat::from_rotation_y(swivel * dt) * (from - to);
        }

        // The further away, the higher.
        let distance = (quick_distance(from.xz(), to.xz()) - Self::CLOSEST).max(0.0);
        let target_y = to.y + distance * Self::HEIGHT_FACTOR + Self::MIN_HEIGHT;
        from.y = self.from.y + (target_y - self.from.y) * Self::FROM_ACCEL_Y * dt;
        // Never below the look-at point, so the camera can't flip over.
        from.y = from.y.max(to.y + Self::MIN_HEIGHT);
        if map.ceiling.is_some() {
            let ceiling = map.height_at(from.x, from.z, LayerKind::Ceiling).0;
            from.y = from.y.min(ceiling - Self::TERRAIN_CLEARANCE);
        }
        from.y = from
            .y
            .max(map.floor_height(from.x, from.z) + Self::TERRAIN_CLEARANCE);

        self.from = from;
        self.to = to;
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.from).looking_at(self.to, Vec3::Y)
    }
}

/// Puts the camera behind the player when the level starts.
/// Port of `InitCamera` (original/src/QD3D/Camera.c).
fn place_follow_camera(
    mut commands: Commands,
    map: Res<TerrainMap>,
    mut camera_angle: ResMut<PlayerToCameraAngle>,
    players: Query<&Transform, With<Player>>,
    cameras: Query<Entity, With<GameCamera>>,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let mut camera = FollowCamera::behind(player.translation, crate::math::yaw_of(player.rotation));
    for _ in 0..FollowCamera::PRIME_UPDATES {
        camera.update(
            player.translation,
            0.0,
            &map,
            &mut camera_angle,
            FollowCamera::PRIME_DT,
        );
    }
    for entity in &cameras {
        commands.entity(entity).remove::<FlyCamera>().insert((
            camera.transform(),
            camera.clone(),
            TransformInterpolation,
        ));
    }
}

/// Moves the camera after the player. Port of `UpdateCamera`.
fn follow_player(
    time: Res<Time>,
    input: Res<ControlInput>,
    map: Res<TerrainMap>,
    mut camera_angle: ResMut<PlayerToCameraAngle>,
    players: Query<&Transform, (With<Player>, Without<GameCamera>)>,
    mut cameras: Query<(&mut FollowCamera, &mut Transform), (With<GameCamera>, Without<FlyCamera>)>,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let dt = time.delta_secs();
    for (mut camera, mut transform) in &mut cameras {
        camera.zoom(&input, dt);
        camera.update(
            player.translation,
            input.camera_swivel(),
            &map,
            &mut camera_angle,
            dt,
        );
        *transform = camera.transform();
    }
}

/// Switches the camera to the debug fly camera and back with F1. The
/// player's controls are off while flying, because they share keys.
fn toggle_fly_camera(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut enabled: ResMut<InputEnabled>,
    cameras: Query<(Entity, &Transform, Has<FlyCamera>), With<GameCamera>>,
) {
    if keys.just_pressed(KeyCode::F1) {
        for (entity, transform, flying) in &cameras {
            if flying {
                commands
                    .entity(entity)
                    .remove::<FlyCamera>()
                    .insert(TransformInterpolation);
            } else {
                let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
                fly(
                    &mut commands,
                    entity,
                    FlyCamera::new(yaw, pitch),
                    *transform,
                );
            }
        }
    }
    let flying = cameras.iter().any(|(_, _, flying)| flying);
    enabled.set_if_neq(InputEnabled(!flying));
}

/// Makes a camera the debug fly camera at `transform`.
pub fn fly(commands: &mut Commands, camera: Entity, fly_camera: FlyCamera, transform: Transform) {
    // The fly camera moves every frame, so it must not be eased between
    // fixed ticks.
    commands
        .entity(camera)
        .remove::<TransformInterpolation>()
        .insert((fly_camera, transform));
}

/// Debug free-flying camera, modelled on Bevy's `bevy_camera_controller` free
/// camera (F1 toggles it). Hold the right mouse button to look around; WASD to move, E and Q
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
