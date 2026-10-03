//! The rolling boulder on the Night level: it waits until a player comes
//! near, then rolls at them, bouncing and gathering speed downhill. Moving
//! fast it hurts players and enemies; slow, it is just a solid rock.
//!
//! Port of `AddRollingBoulder` and `MoveRollingBoulder`
//! (original/src/Items/Traps.c).

use std::f32::consts::TAU;

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::super::kind as item;
use super::super::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource,
};
use super::TrapModel;
use crate::collision::{
    BoxMover, CollisionBox, CollisionCandidates, CollisionKind, CollisionSystems, SolidSides,
    resolve_box_collisions, solid_object,
};
use crate::combat::Damage;
use crate::enemies::{ORIGINAL_FRAME_RATE, apply_friction};
use crate::math::{GameRandom, turn_toward};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::state::AppState;
use crate::terrain::{LayerKind, TerrainMap};

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::ROLLING_BOULDER, add_rolling_boulder)
        .add_systems(
            FixedUpdate,
            // The boulder is early in the object list (slot 50, before the
            // player's 200): it rolls before the player moves, so that the
            // player's collision sees where it rolled this tick.
            roll_boulders
                .after(ItemSystems::Track)
                .before(CollisionSystems::Gather)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `GLOBAL1_MObjType_ThrowRock`
const BOULDER_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 6);
/// `BOULDER_RADIUS`
const BOULDER_RADIUS: f32 = 35.0;
/// `BOULDER_SCALE`
const BOULDER_SCALE: f32 = 3.0;
/// Half the size of the boulder's box, in units.
const BOULDER_SIZE: f32 = BOULDER_RADIUS * BOULDER_SCALE;
/// How high above the floor a new boulder is put, before its first move
/// lifts it to [`BOULDER_SIZE`].
const BOULDER_START_HEIGHT: f32 = 30.0 * BOULDER_SCALE;
/// How close a player has to come to set it rolling (`BOULDER_DIST`).
const BOULDER_DIST: f32 = 1200.0;
/// How fast it sets off toward the player, in units per second.
const BOULDER_START_SPEED: f32 = 200.0;
/// Below this damage it doesn't hurt (`BOULDER_MIN_DAMAGE`).
const BOULDER_MIN_DAMAGE: f32 = 0.1;
/// `BOULDER_MAX_DAMAGE`
const BOULDER_MAX_DAMAGE: f32 = 0.4;
/// Its damage before it moves (`Damage`), which only counts once rolling.
const BOULDER_START_DAMAGE: f32 = 0.3;
/// The speed, in units per second, that deals one unit of damage.
const DAMAGE_SPEED: f32 = 2700.0;
/// Gravity, in units per second squared.
const BOULDER_GRAVITY: f32 = 1400.0;
/// Slowing on each horizontal axis, in units per second squared.
const BOULDER_FRICTION: f32 = 30.0;
/// How much of its fall speed a bounce keeps.
const BOUNCE: f32 = 0.8;
/// Bounces slower than this, in units per second, stop.
const BOUNCE_STOP: f32 = 1.0;
/// Bounces faster than this, in units per second, slam.
const BOUNCE_SLAM: f32 = 150.0;
/// The highest bounce, in units per second.
const BOUNCE_MAX: f32 = 400.0;
/// Floors flatter than this (the normal's y) don't pull the boulder.
const FLAT_FLOOR: f32 = 0.95;
/// How hard a slope pulls the boulder, in units per second squared per
/// unit of the floor normal.
const SLOPE_PULL: f32 = 900.0;
/// How fast the boulder spins about its axle, in radians per unit
/// travelled.
const ROLL_PER_UNIT: f32 = 0.01;
/// How fast it turns its axle to its path, in radians per second per unit
/// per second of speed, at the original's 60 frames per second.
const TURN_PER_SPEED: f32 = 0.5 / ORIGINAL_FRAME_RATE;

/// The boulder's box, around its centre.
const BOULDER_BOX: CollisionBox = CollisionBox::new(
    BOULDER_SIZE,
    -BOULDER_SIZE,
    -BOULDER_SIZE,
    BOULDER_SIZE,
    BOULDER_SIZE,
    -BOULDER_SIZE,
);

/// A boulder's state (`BoulderIsActive`, and its `Rot`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct RollingBoulder {
    /// Rolling; until then it waits for a player.
    active: bool,
    /// The way its axle faces.
    yaw: f32,
    /// How far it has spun about its axle.
    roll: f32,
}

/// The collision kinds a boulder always has.
fn base_kinds() -> LayerMask {
    LayerMask::from([
        CollisionKind::Misc,
        CollisionKind::BlockShadow,
        CollisionKind::BlockCamera,
    ])
}

/// Port of `AddRollingBoulder`.
fn add_rolling_boulder(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
) -> bool {
    let (x, z) = (spawn.position.x, spawn.position.y);
    let position = Vec3::new(x, map.floor_height(x, z) + BOULDER_START_HEIGHT, z);
    let yaw = random.next_f32() * TAU;
    let root = commands
        .spawn((
            Name::new("Rolling boulder"),
            Transform::from_translation(position),
            Visibility::default(),
            TransformInterpolation,
            PreviousPosition(position),
            Velocity::default(),
            CollisionCandidates::default(),
            RollingBoulder {
                active: false,
                yaw,
                roll: 0.0,
            },
            Damage(BOULDER_START_DAMAGE),
            solid_object(vec![BOULDER_BOX], base_kinds(), SolidSides::ALL),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    if let Some(model) = models.spawn(
        &mut commands,
        root,
        BOULDER_MODEL,
        Shading::Lit,
        Transform::from_rotation(Quat::from_rotation_y(yaw)).with_scale(Vec3::splat(BOULDER_SCALE)),
    ) {
        commands.entity(root).insert(TrapModel(model));
    }
    true
}

/// The velocity a waiting boulder at `at` sets off with toward the nearest
/// player, if one is close enough.
fn wake_velocity(at: Vec2, players: impl IntoIterator<Item = Vec2>) -> Option<Vec3> {
    let (player, d) = players
        .into_iter()
        .map(|p| (p, at.distance(p)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    // The original divides by the distance; a player right on it can't
    // set the direction.
    if d >= BOULDER_DIST || d <= 0.0 {
        return None;
    }
    let v = (player - at) * (BOULDER_START_SPEED / d);
    Some(Vec3::new(v.x, 0.0, v.y))
}

/// The upward speed after landing while moving at `vy`.
fn land(vy: f32) -> f32 {
    if vy >= 0.0 {
        // Hit while going up a slope.
        return 0.0;
    }
    let up = -vy * BOUNCE;
    if up.abs() < BOUNCE_STOP {
        0.0
    } else if up > BOUNCE_SLAM {
        // Sound: EFFECT_ROCKSLAM at the boulder.
        up.min(BOUNCE_MAX)
    } else {
        up
    }
}

/// The damage a boulder rolling at `speed` (units per second) deals.
fn rolling_damage(speed: f32) -> f32 {
    (speed / DAMAGE_SPEED).min(BOULDER_MAX_DAMAGE)
}

/// Rolls one active boulder for `dt` seconds: gravity, friction, motion,
/// the floor and its slopes. Returns its speed over the ground.
fn roll(coord: &mut Vec3, velocity: &mut Vec3, map: &TerrainMap, dt: f32) -> f32 {
    velocity.y -= BOULDER_GRAVITY * dt;
    apply_friction(velocity, BOULDER_FRICTION, dt);
    *coord += *velocity * dt;

    let (floor, normal) = map.height_at(coord.x, coord.z, LayerKind::Floor);
    if coord.y - BOULDER_SIZE < floor {
        coord.y = floor + BOULDER_SIZE;
        velocity.y = land(velocity.y);
    }
    if normal.y < FLAT_FLOOR {
        velocity.x += normal.x * dt * SLOPE_PULL;
        velocity.z += normal.z * dt * SLOPE_PULL;
    }
    velocity.xz().length()
}

/// Port of `MoveRollingBoulder` (original/src/Items/Traps.c). Being
/// deleted out of range (`TrackTerrainItem`) is [`DespawnOutOfRange`].
///
/// It collides with what the last tick's [`CollisionSystems::Gather`]
/// found around it, as it moves before this tick's.
///
/// The original turns the axle by its speed times the frame time twice
/// over (`TurnObjectTowardTarget` scales the turn speed it is given by the
/// frame time again), so how fast it turns depends on the frame rate; this
/// turns as the original does at 60 frames per second.
fn roll_boulders(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut boulders: Query<(
        Entity,
        &mut Transform,
        &mut Velocity,
        &PreviousPosition,
        &CollisionCandidates,
        &mut RollingBoulder,
        &mut Damage,
        &CollisionLayers,
        &mut SolidSides,
        Option<&TrapModel>,
    )>,
    players: Query<&Transform, (With<Player>, Without<RollingBoulder>)>,
    mut models: Query<&mut Transform, (Without<RollingBoulder>, Without<Player>)>,
) {
    let dt = time.delta_secs();
    for (
        entity,
        mut transform,
        mut velocity,
        previous,
        candidates,
        mut boulder,
        mut damage,
        layers,
        mut solid,
        model,
    ) in &mut boulders
    {
        let mut coord = transform.translation;
        let mut delta = **velocity;
        if boulder.active {
            let old = coord.xz();
            let speed = roll(&mut coord, &mut delta, &map, dt);
            // The axle turns to face back where it came from.
            boulder.yaw = turn_toward(boulder.yaw, coord.xz(), old, speed * TURN_PER_SPEED * dt).0;
            boulder.roll += speed * dt * ROLL_PER_UNIT;

            **damage = rolling_damage(speed);
            // Moving fast enough to hurt, it is no longer solid.
            let (kinds, sides) = if **damage >= BOULDER_MIN_DAMAGE {
                let hurts = [CollisionKind::HurtMe, CollisionKind::HurtEnemy];
                (base_kinds() | LayerMask::from(hurts), SolidSides::TOUCHABLE)
            } else {
                (base_kinds(), SolidSides::ALL)
            };
            solid.set_if_neq(sides);
            if layers.memberships != kinds {
                commands
                    .entity(entity)
                    .insert(CollisionLayers::new(kinds, LayerMask::NONE));
            }

            let mover = BoxMover {
                entity,
                is_player: false,
                shape: BOULDER_BOX,
                old_coord: **previous,
                platform_velocity: Vec3::ZERO,
            };
            resolve_box_collisions(
                &mover,
                &mut coord,
                &mut delta,
                CollisionKind::Misc.into(),
                &candidates.0,
                dt,
                &mut EntityHashSet::default(),
            );
        } else {
            coord.y = map.floor_height(coord.x, coord.z) + BOULDER_SIZE;
            if let Some(start) =
                wake_velocity(coord.xz(), players.iter().map(|p| p.translation.xz()))
            {
                boulder.active = true;
                delta.x = start.x;
                delta.z = start.z;
            }
        }
        transform.translation = coord;
        **velocity = delta;
        if let Some(mut model) = model.and_then(|m| models.get_mut(m.0).ok()) {
            model.rotation = Quat::from_euler(EulerRot::ZYX, 0.0, boulder.yaw, boulder.roll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boulder_wakes_for_the_nearest_player_in_range() {
        let at = Vec2::ZERO;
        assert_eq!(wake_velocity(at, [Vec2::new(1300.0, 0.0)]), None);
        assert_eq!(wake_velocity(at, []), None);
        let v = wake_velocity(at, [Vec2::new(0.0, 1100.0), Vec2::new(-600.0, 0.0)]);
        assert_eq!(v, Some(Vec3::new(-BOULDER_START_SPEED, 0.0, 0.0)));
    }

    #[test]
    fn landing_bounces_less_each_time_and_not_too_high() {
        assert_eq!(land(-100.0), 80.0);
        assert_eq!(land(-1000.0), BOUNCE_MAX);
        assert_eq!(land(-1.0), 0.0);
        // Going up a slope it just follows the floor.
        assert_eq!(land(50.0), 0.0);
    }

    #[test]
    fn a_boulder_hurts_more_the_faster_it_rolls() {
        assert!(rolling_damage(100.0) < BOULDER_MIN_DAMAGE);
        assert!((rolling_damage(540.0) - 0.2).abs() < 1e-6);
        assert_eq!(rolling_damage(5000.0), BOULDER_MAX_DAMAGE);
    }

    #[test]
    fn a_rolling_boulder_stays_on_the_floor_and_slows_on_the_flat() {
        let map = TerrainMap::load_for_tests("Night", false);
        let (x, z) = (5648.0 * 2.0, 584.0 * 2.0);
        let mut coord = Vec3::new(x, map.floor_height(x, z) + BOULDER_SIZE, z);
        let mut velocity = Vec3::new(200.0, 0.0, 0.0);
        for _ in 0..120 {
            roll(&mut coord, &mut velocity, &map, 1.0 / 60.0);
            assert!(coord.y >= map.floor_height(coord.x, coord.z) + BOULDER_SIZE - 1e-2);
        }
    }
}
