//! The queen bee's spit: a blob of honey that she spits at the player. It
//! bounces along the floor, wobbling and growing, and once fully grown
//! hatches an enemy. It is viscous (`CTYPE_VISCOUS`), which slows a player
//! inside it. After [`SPIT_LIFETIME`] it sinks into the floor and is gone.
//!
//! Port of `ShootSpit` and `MoveQueenSpit`
//! (original/src/Enemies/Enemy_QueenBee.c).

use avian3d::prelude::TransformInterpolation;
use bevy::prelude::*;

use super::QUEEN_BEE_HEALTH;
use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::combat::Health;
use crate::enemies::flying_bee::make_flying_bee;
use crate::enemies::larva::make_larva_enemy;
use crate::enemies::{Bosses, EnemySpawner, ORIGINAL_FRAME_RATE, apply_friction};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::state::AppState;
use crate::terrain::TerrainMap;

/// `HIVE_MObjType_HoneyBlob`
const HONEY_BLOB_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 24);
/// The blob's size when spat (`scale = .15`).
const SPIT_START_SCALE: f32 = 0.15;
/// The size at which it hatches, and stops growing.
const SPIT_HATCH_SCALE: f32 = 2.2;
/// How fast it grows, in scale per second.
const SPIT_GROWTH: f32 = 0.5;
/// How far its size wobbles, as a share of its size.
const SPIT_WOBBLE: f32 = 0.2;
/// How fast each axis's wobble angle turns, in radians per second.
const SPIT_WOBBLE_SPEED: Vec3 = Vec3::new(4.5, -5.0, 4.0);

/// How fast it is spat forward, in units per second (`SPIT_SPEED`).
const SPIT_SPEED: f32 = 500.0;
/// How fast it is spat upward, in units per second.
const SPIT_RISE: f32 = 250.0;
/// Gravity on it, in units per second squared.
const SPIT_GRAVITY: f32 = 800.0;
/// Friction on it, per frame at 60 fps.
const SPIT_FRICTION_PER_FRAME: f32 = 5.0;
/// How far above the floor it rests, in units.
const SPIT_FLOOR_CLEARANCE: f32 = 20.0;
/// How much of its fall it bounces back up with.
const SPIT_BOUNCE: f32 = 0.3;
/// Seconds before it starts to sink away (`SpitTimer = 80`).
const SPIT_LIFETIME: f32 = 80.0;
/// How fast it sinks away, in units per second.
const SPIT_SINK_SPEED: f32 = 40.0;
/// How far below the floor it sinks before it is gone, in units.
const SPIT_SINK_DEPTH: f32 = 400.0;
/// Its collision box, which keeps its size as the blob grows.
const SPIT_BOX: CollisionBox = CollisionBox::new(300.0, 0.0, -200.0, 200.0, 200.0, -200.0);

/// The queen spat a blob from `at`, facing `yaw`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct SpitShot {
    pub at: Vec3,
    pub yaw: f32,
}

/// A blob of honey the queen spat.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct QueenSpit {
    /// Its model, which carries its wobbling scale.
    model: Option<Entity>,
    /// Seconds before it sinks away (`SpitTimer`).
    lifetime: f32,
    /// It has hatched its enemy (`HasSpawned`).
    has_spawned: bool,
    /// The wobble angles of x, y and z (`WobbleX`, `WobbleY`, `WobbleZ`).
    wobble: Vec3,
    /// Its size, which the wobble goes round (`WobbleBase`).
    base_scale: f32,
}

impl QueenSpit {
    fn new(model: Option<Entity>) -> Self {
        Self {
            model,
            lifetime: SPIT_LIFETIME,
            has_spawned: false,
            wobble: Vec3::ZERO,
            base_scale: SPIT_START_SCALE,
        }
    }

    /// Wobbles and grows for `dt` seconds, and returns the scale to draw
    /// it at (from the size before it grew) and whether it is time to
    /// hatch, which happens once.
    fn grow(&mut self, dt: f32) -> (Vec3, bool) {
        let base = self.base_scale;
        self.wobble += SPIT_WOBBLE_SPEED * dt;
        let scale = Vec3::new(
            base + self.wobble.x.sin() * SPIT_WOBBLE * base,
            base + self.wobble.y.sin() * SPIT_WOBBLE * base,
            base + self.wobble.z.cos() * SPIT_WOBBLE * base,
        );
        if self.base_scale < SPIT_HATCH_SCALE {
            self.base_scale += SPIT_GROWTH * dt;
            (scale, false)
        } else if !self.has_spawned {
            self.has_spawned = true;
            (scale, true)
        } else {
            (scale, false)
        }
    }
}

/// The velocity a blob spat by a queen facing `yaw` starts with.
fn launch(yaw: f32) -> Vec3 {
    Vec3::new(-yaw.sin() * SPIT_SPEED, SPIT_RISE, -yaw.cos() * SPIT_SPEED)
}

/// Spawns the blobs the queen spat.
///
/// Port of `ShootSpit` (original/src/Enemies/Enemy_QueenBee.c).
pub(super) fn shoot_spit(
    mut shots: MessageReader<SpitShot>,
    mut commands: Commands,
    mut enemies: EnemySpawner,
) {
    for shot in shots.read() {
        let spit = commands
            .spawn((
                Name::new("Queen bee's spit"),
                Transform::from_translation(shot.at),
                Visibility::default(),
                TransformInterpolation,
                PreviousPosition(shot.at),
                Velocity(launch(shot.yaw)),
                solid_object(
                    vec![SPIT_BOX],
                    CollisionKind::Viscous,
                    SolidSides::TOUCHABLE,
                ),
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        let model = enemies.models().spawn(
            &mut commands,
            spit,
            HONEY_BLOB_MODEL,
            Shading::Lit,
            Transform::from_scale(Vec3::splat(SPIT_START_SCALE)),
        );
        if let Some(model) = model {
            commands.entity(model).insert(TransformInterpolation);
        }
        commands.entity(spit).insert(QueenSpit::new(model));
    }
}

/// Moves an active blob for `dt` seconds: friction, gravity, and a bounce
/// off `floor` (the floor's height where it ends up).
fn bounce(coord: &mut Vec3, velocity: &mut Vec3, floor: impl Fn(Vec3) -> f32, dt: f32) {
    apply_friction(velocity, SPIT_FRICTION_PER_FRAME * ORIGINAL_FRAME_RATE, dt);
    velocity.y -= SPIT_GRAVITY * dt;
    *coord += *velocity * dt;
    let rest = floor(*coord) + SPIT_FLOOR_CLEARANCE;
    if coord.y < rest {
        velocity.y *= -SPIT_BOUNCE;
        coord.y = rest;
    }
}

/// Whether a queen with `health` left hatches a flying bee rather than a
/// larva: once she has lost half her health. With no queen, her health
/// counts as none.
fn hatches_flying_bee(health: Option<f32>) -> bool {
    health.unwrap_or(0.0) < QUEEN_BEE_HEALTH / 2.0
}

/// Moves the blobs: an active one bounces, wobbles, grows and hatches its
/// enemy; an old one sinks away.
///
/// Port of `MoveQueenSpit` (original/src/Enemies/Enemy_QueenBee.c).
#[allow(clippy::too_many_arguments)]
pub(super) fn move_queen_spit(
    time: Res<Time>,
    map: Res<TerrainMap>,
    bosses: Res<Bosses>,
    healths: Query<&Health>,
    mut random: ResMut<GameRandom>,
    mut enemies: EnemySpawner,
    mut spits: Query<(Entity, &mut Transform, &mut Velocity, &mut QueenSpit)>,
    mut models: Query<&mut Transform, Without<QueenSpit>>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut velocity, mut spit) in &mut spits {
        let mut coord = transform.translation;
        spit.lifetime -= dt;
        if spit.lifetime < 0.0 {
            coord.y -= SPIT_SINK_SPEED * dt;
            if coord.y < map.floor_height(coord.x, coord.z) - SPIT_SINK_DEPTH {
                enemies.commands().entity(entity).despawn();
                continue;
            }
            transform.translation = coord;
            continue;
        }

        bounce(
            &mut coord,
            &mut velocity,
            |at| map.floor_height(at.x, at.z),
            dt,
        );
        transform.translation = coord;
        let (scale, hatch) = spit.grow(dt);
        if let Some(mut model) = spit.model.and_then(|m| models.get_mut(m).ok()) {
            model.scale = scale;
        }
        if hatch {
            let queen_health = bosses
                .queen_bee
                .and_then(|queen| healths.get(queen).ok())
                .map(|h| h.0);
            if hatches_flying_bee(queen_health) {
                make_flying_bee(&mut enemies, &mut random, coord);
            } else {
                make_larva_enemy(&mut enemies, &mut random, coord.xz());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_is_spat_ahead_and_up() {
        let velocity = launch(0.0);
        assert!((velocity - Vec3::new(0.0, SPIT_RISE, -SPIT_SPEED)).length() < 1e-3);
    }

    #[test]
    fn a_blob_bounces_off_the_floor() {
        let mut coord = Vec3::new(0.0, SPIT_FLOOR_CLEARANCE + 1.0, 0.0);
        let mut velocity = Vec3::new(0.0, -600.0, 0.0);
        bounce(&mut coord, &mut velocity, |_| 0.0, 0.1);
        assert_eq!(coord.y, SPIT_FLOOR_CLEARANCE);
        assert!(velocity.y > 0.0);
        assert!((velocity.y - (600.0 + SPIT_GRAVITY * 0.1) * SPIT_BOUNCE).abs() < 1e-3);
    }

    #[test]
    fn a_blob_grows_for_about_four_seconds_then_hatches_once() {
        let mut spit = QueenSpit::new(None);
        let dt = 1.0 / 60.0;
        let mut hatched_at = None;
        let mut hatches = 0;
        for tick in 0..600 {
            let (scale, hatch) = spit.grow(dt);
            assert!(scale.min_element() > 0.0);
            if hatch {
                hatches += 1;
                hatched_at.get_or_insert(tick as f32 * dt);
            }
        }
        assert_eq!(hatches, 1);
        let seconds = (SPIT_HATCH_SCALE - SPIT_START_SCALE) / SPIT_GROWTH;
        let hatched_at = hatched_at.unwrap_or_default();
        assert!((hatched_at - seconds).abs() < 0.1, "{hatched_at}");
        assert!(spit.base_scale >= SPIT_HATCH_SCALE);
    }

    #[test]
    fn a_hurt_queen_hatches_flying_bees() {
        assert!(!hatches_flying_bee(Some(QUEEN_BEE_HEALTH)));
        assert!(!hatches_flying_bee(Some(QUEEN_BEE_HEALTH / 2.0)));
        assert!(hatches_flying_bee(Some(QUEEN_BEE_HEALTH / 2.0 - 0.1)));
        assert!(hatches_flying_bee(None));
    }
}
