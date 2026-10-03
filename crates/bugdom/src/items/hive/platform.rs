//! Honeycomb platforms: brick ones that drop away once stood on, solid
//! steel ones, and wooden ones that travel along a spline.
//!
//! Port of `AddHoneycombPlatform`, `MoveHoneycombPlatform` and
//! `DoTrig_HoneycombPlatform` (original/src/Items/Triggers.c), and of
//! `PrimeHoneycombPlatform` and `MoveHoneycombPlatformOnSpline`
//! (original/src/Items/Items2.c).
//!
//! The spline platforms are moving platforms (`CTYPE_MPLATFORM`): the
//! player's controller carries a player standing on one by its
//! [`Velocity`] (`MPlatform->Delta`).

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::prelude::*;

use super::model;
use crate::collision::{
    CollisionBox, CollisionKind, CollisionSystems, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::items::kind as item;
use crate::items::scenery::on_level;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource,
};
use crate::level::{CurrentLevel, LevelType};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::PlayerSystems;
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::HONEYCOMB_PLATFORM, add_honeycomb_platform)
        .register_spline_item_kind(item::HONEYCOMB_PLATFORM, prime_honeycomb_platform)
        .add_systems(
            FixedUpdate,
            (
                // Triggers move early in the original's object list, before
                // the player (`TRIGGER_SLOT`).
                move_honeycomb_platforms
                    .after(ItemSystems::Track)
                    .before(CollisionSystems::Gather)
                    .before(PlayerSystems::Move),
                drop_honeycomb_platforms.after(PlayerSystems::Move),
                move_platforms_on_spline.in_set(SplineSystems::Move),
            )
                .run_if(in_state(AppState::InGame)),
        );
}

/// `HONEYCOMB_PLATFORM_SCALE` and, for small ones, `HONEYCOMB_PLATFORM_SCALE2`.
const PLATFORM_SCALE: f32 = 2.0;
const SMALL_PLATFORM_SCALE: f32 = 1.3;
/// The elevation of a platform whose `params[1]` is 0.
const DEFAULT_ELEVATION: f32 = 11.0;
/// How far down each step of elevation puts a platform, in units. The
/// height is absolute, not above the floor.
const ELEVATION_STEP: f32 = -50.0;
/// How fast a dropping platform speeds up, in units per second².
const FALL_GRAVITY: f32 = 120.0;
/// How far under the floor a dropping platform goes before it is gone or
/// comes back up, in units.
const SINK_DEPTH: f32 = 200.0;
/// How fast a platform comes back up, in units per second.
const RISE_SPEED: f32 = 50.0;
/// How fast the wooden platforms travel along their splines, in baked
/// points per second.
const SPLINE_SPEED: f32 = 90.0;

/// `params[0]` bit 0: a steel platform, which never drops.
const PARAM0_METAL: u8 = 1;
/// `params[3]` bits: a dropped platform comes back up; the platform is
/// small; a spline platform goes back and forth.
const PARAM3_RESURFACE: u8 = 1;
const PARAM3_SMALL: u8 = 1 << 1;
const PARAM3_ZIGZAG: u8 = 1 << 2;

/// A brick platform that drops away once the player stands on it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct HoneycombPlatform {
    pub state: PlatformState,
    /// Whether it comes back up after dropping (`ResurfacePlatform`).
    pub resurface: bool,
    /// Its height when in place (`InitCoord.y`).
    pub home_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformState {
    /// In place, waiting to be stood on.
    Normal,
    Fall,
    Rise,
}

/// What a step of a dropping or rising platform did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlatformStep {
    Moved,
    /// It went under the floor and starts coming back up; it can be set off
    /// again.
    Resurfacing,
    /// It went under the floor for good.
    Gone,
}

impl HoneycombPlatform {
    /// Moves the platform for `dt` seconds. `velocity` is its vertical
    /// velocity, kept between states as the original's `Delta.y` is.
    ///
    /// The `Mode` switch of `MoveHoneycombPlatform`.
    fn step(&mut self, y: &mut f32, velocity: &mut f32, floor: f32, dt: f32) -> PlatformStep {
        match self.state {
            PlatformState::Normal => {}
            PlatformState::Fall => {
                *velocity -= FALL_GRAVITY * dt;
                *y += *velocity * dt;
                if *y < floor - SINK_DEPTH {
                    if !self.resurface {
                        return PlatformStep::Gone;
                    }
                    self.state = PlatformState::Rise;
                    *velocity = RISE_SPEED;
                    return PlatformStep::Resurfacing;
                }
            }
            PlatformState::Rise => {
                *y += *velocity * dt;
                if *y >= self.home_y {
                    *y = self.home_y;
                    self.state = PlatformState::Normal;
                }
            }
        }
        PlatformStep::Moved
    }
}

/// A wooden platform travelling along a spline.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplinePlatform {
    /// Goes back and forth instead of round (`ZigZag`).
    zigzag: bool,
}

/// A platform's scale and box for its `params[3]`.
fn platform_shape(params: [u8; 4]) -> (f32, CollisionBox) {
    let s = if params[3] & PARAM3_SMALL != 0 {
        SMALL_PLATFORM_SCALE
    } else {
        PLATFORM_SCALE
    };
    let shape = CollisionBox::new(
        65.0 * s,
        -300.0 * s,
        -200.0 * s,
        200.0 * s,
        200.0 * s,
        -200.0 * s,
    );
    (s, shape)
}

/// A platform's height for its `params[1]`.
fn platform_height(params: [u8; 4]) -> f32 {
    let h = if params[1] == 0 {
        DEFAULT_ELEVATION
    } else {
        f32::from(params[1])
    };
    h * ELEVATION_STEP
}

/// What a brick platform is while it can be set off; without `Trigger`
/// once it has been.
fn brick_kinds(trigger: bool) -> CollisionLayers {
    let mut kinds = LayerMask::from([
        CollisionKind::BlockShadow,
        CollisionKind::Impenetrable,
        CollisionKind::Impenetrable2,
        CollisionKind::PlayerTriggerOnly,
        CollisionKind::Misc,
        CollisionKind::BlockCamera,
    ]);
    if trigger {
        kinds |= CollisionKind::Trigger;
    }
    CollisionLayers::new(kinds, LayerMask::NONE)
}

/// Port of `AddHoneycombPlatform`. `params[0]` bit 0 makes it steel,
/// `params[1]` is its elevation, and `params[3]` bit 0 makes a brick one
/// come back after dropping and bit 1 makes it small.
fn add_honeycomb_platform(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive], "Honeycomb platform") {
        return false;
    }
    let metal = spawn.params[0] & PARAM0_METAL != 0;
    let (scale, shape) = platform_shape(spawn.params);
    let y = platform_height(spawn.params);
    let position = Vec3::new(spawn.position.x, y, spawn.position.y);

    let platform = commands
        .spawn((
            Name::new("Honeycomb platform"),
            Transform::from_translation(position),
            Visibility::default(),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    if metal {
        commands.entity(platform).insert(solid_object(
            vec![shape],
            [
                CollisionKind::BlockShadow,
                CollisionKind::Misc,
                CollisionKind::Impenetrable,
                CollisionKind::Impenetrable2,
            ],
            SolidSides::ALL,
        ));
    } else {
        commands.entity(platform).insert((
            solid_object(vec![shape], LayerMask::NONE, SolidSides::ALL),
            brick_kinds(true),
            Trigger {
                sides: SolidSides::TOP,
                solid: true,
            },
            HoneycombPlatform {
                state: PlatformState::Normal,
                resurface: spawn.params[3] & PARAM3_RESURFACE != 0,
                home_y: y,
            },
            Velocity::default(),
            PreviousPosition(position),
            TransformInterpolation,
        ));
    }
    models.spawn(
        &mut commands,
        platform,
        ModelRef::new(
            ModelFile::Level1,
            model::BRICK_PLATFORM + usize::from(metal),
        ),
        Shading::Lit,
        Transform::from_scale(Vec3::splat(scale)),
    );
    true
}

/// Drops and raises the brick platforms. Port of `MoveHoneycombPlatform`;
/// leaving the item window (`TrackTerrainItem`) is [`DespawnOutOfRange`].
/// A platform gone under the floor for good is deleted with its item, so
/// it comes back once its place scrolls into view again, as in the
/// original.
fn move_honeycomb_platforms(
    time: Res<Time>,
    mut commands: Commands,
    map: Res<TerrainMap>,
    mut platforms: Query<(
        Entity,
        &mut HoneycombPlatform,
        &mut Transform,
        &mut Velocity,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut platform, mut transform, mut velocity) in &mut platforms {
        let position = transform.translation;
        let floor = map.floor_height(position.x, position.z);
        let mut y = position.y;
        match platform.step(&mut y, &mut velocity.y, floor, dt) {
            PlatformStep::Moved => {}
            PlatformStep::Resurfacing => {
                commands.entity(entity).insert(brick_kinds(true));
            }
            PlatformStep::Gone => {
                commands.entity(entity).despawn();
                continue;
            }
        }
        if y != position.y {
            transform.translation.y = y;
        }
    }
}

/// A brick platform the player has landed on starts to drop and can't be
/// set off again until it comes back up. Port of
/// `DoTrig_HoneycombPlatform`.
fn drop_honeycomb_platforms(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut platforms: Query<&mut HoneycombPlatform>,
) {
    for hit in hits.read() {
        let Ok(mut platform) = platforms.get_mut(hit.trigger) else {
            continue;
        };
        platform.state = PlatformState::Fall;
        commands.entity(hit.trigger).insert(brick_kinds(false));
    }
}

/// Port of `PrimeHoneycombPlatform`. `params[1]` is its elevation, and
/// `params[3]` bit 1 makes it small and bit 2 makes it go back and forth.
fn prime_honeycomb_platform(
    In(spawn): In<SplineItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
) -> bool {
    let (scale, shape) = platform_shape(spawn.params);
    let position = Vec3::new(
        spawn.position.x,
        platform_height(spawn.params),
        spawn.position.y,
    );
    let platform = commands
        .spawn((
            Name::new("Spline platform"),
            Transform::from_translation(position),
            OnSpline::new(spawn.spline, spawn.placement, SPLINE_SPEED),
            SplinePlatform {
                zigzag: spawn.params[3] & PARAM3_ZIGZAG != 0,
            },
            solid_object(
                vec![shape],
                [
                    CollisionKind::Misc,
                    CollisionKind::MovingPlatform,
                    CollisionKind::BlockCamera,
                    CollisionKind::Impenetrable,
                    CollisionKind::BlockShadow,
                    CollisionKind::Impenetrable2,
                ],
                SolidSides::ALL,
            ),
            Velocity::default(),
            PreviousPosition(position),
            TransformInterpolation,
        ))
        .id();
    models.spawn(
        &mut commands,
        platform,
        model::WOOD_PLATFORM,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(scale)),
    );
    true
}

/// Moves the wooden platforms along their splines. While a platform is in
/// view its velocity is its motion this tick, which is what riders and
/// colliders see (`Delta`); out of view the original leaves its velocity
/// alone. Port of `MoveHoneycombPlatformOnSpline`.
///
/// As in the original, platforms move after the player, with the other
/// spline objects.
fn move_platforms_on_spline(
    time: Res<Time>,
    splines: Option<Res<Splines>>,
    mut platforms: Query<(
        &SplinePlatform,
        &mut OnSpline,
        &mut Transform,
        &mut Velocity,
    )>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (platform, mut on_spline, mut transform, mut velocity) in &mut platforms {
        let old = transform.translation;
        if platform.zigzag {
            on_spline.advance_zigzag(&splines, dt);
        } else {
            on_spline.advance(&splines, dt);
        }
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;
        if on_spline.visible && dt > 0.0 {
            **velocity = (transform.translation - old) / dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brick(resurface: bool) -> HoneycombPlatform {
        HoneycombPlatform {
            state: PlatformState::Fall,
            resurface,
            home_y: -550.0,
        }
    }

    #[test]
    fn heights_and_shapes_follow_the_params() {
        assert_eq!(platform_height([0, 0, 0, 0]), -550.0);
        assert_eq!(platform_height([0, 4, 0, 0]), -200.0);
        let (scale, shape) = platform_shape([0, 0, 0, 0]);
        assert_eq!(scale, 2.0);
        assert_eq!(shape.top, 130.0);
        assert_eq!(shape.bottom, -600.0);
        let (scale, shape) = platform_shape([0, 0, 0, PARAM3_SMALL]);
        assert_eq!(scale, 1.3);
        assert!((shape.right - 260.0).abs() < 1e-3);
    }

    #[test]
    fn a_normal_platform_stays_put() {
        let mut platform = brick(false);
        platform.state = PlatformState::Normal;
        let (mut y, mut v) = (-550.0, 0.0);
        assert_eq!(
            platform.step(&mut y, &mut v, -800.0, 0.1),
            PlatformStep::Moved
        );
        assert_eq!(y, -550.0);
    }

    #[test]
    fn a_dropping_platform_speeds_up_and_is_gone_under_the_floor() {
        let mut platform = brick(false);
        let (mut y, mut v) = (-550.0, 0.0);
        let floor = -600.0;
        assert_eq!(
            platform.step(&mut y, &mut v, floor, 1.0),
            PlatformStep::Moved
        );
        assert_eq!(v, -120.0);
        assert_eq!(y, -670.0);
        let mut steps = 0;
        while platform.step(&mut y, &mut v, floor, 0.1) == PlatformStep::Moved {
            steps += 1;
            assert!(steps < 1000);
        }
        assert!(y < floor - SINK_DEPTH);
    }

    #[test]
    fn a_resurfacing_platform_comes_back_to_its_place() {
        let mut platform = brick(true);
        let (mut y, mut v) = (-550.0, 0.0);
        let floor = -600.0;
        let mut step = PlatformStep::Moved;
        while step == PlatformStep::Moved {
            step = platform.step(&mut y, &mut v, floor, 0.1);
        }
        assert_eq!(step, PlatformStep::Resurfacing);
        assert_eq!(platform.state, PlatformState::Rise);
        assert_eq!(v, RISE_SPEED);
        while platform.state == PlatformState::Rise {
            platform.step(&mut y, &mut v, floor, 0.1);
        }
        assert_eq!(platform.state, PlatformState::Normal);
        assert_eq!(y, -550.0);
        // The rise speed stays, as the original's `Delta.y` does.
        assert_eq!(v, RISE_SPEED);
    }
}
