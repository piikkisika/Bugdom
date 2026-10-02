//! The skippy: a water skimmer on the pond that darts at the player in
//! spurts. It hurts on contact (it is spiked) and leaves ripples behind.
//!
//! Port of original/src/Enemies/Enemy_Skippy.c. It is both a map item and
//! a spline item ([`kind::SKIPPY`]). A skippy on a spline leaves it to
//! chase the player once the player comes in range.
//!
//! It isn't kickable, the ball doesn't hit it (`PlayerHitEnemy` has no
//! skippy case) and its spikes are the player's collision, so it answers
//! only [`EnemyKilled`].

use bevy::prelude::*;

use super::{
    EnemyBody, EnemyCollision, EnemyKilled, EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner,
    EnemySystems, MAX_ENEMIES, default_enemy_collision_mask, detach_enemy_from_spline,
    nearest_player,
};
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::effects::{RippleMaker, make_ripple};
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::skeleton::{AnimationFlags, SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::terrain::TerrainMap;

pub struct SkippyPlugin;

impl Plugin for SkippyPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::SKIPPY, add_skippy)
            .register_spline_item_kind(kind::SKIPPY, prime_skippy)
            .add_systems(
                FixedUpdate,
                (
                    move_skippies.in_set(EnemySystems::Move),
                    kill_hurt_skippies.in_set(EnemySystems::Killed),
                    move_skippies_on_spline.in_set(SplineSystems::Move),
                ),
            );
    }
}

/// The pond's water surface (`WATER_Y`).
const WATER_Y: f32 = 0.0;
/// The height a swimming skippy keeps (`SKIPPY_Y`).
const SKIPPY_Y: f32 = WATER_Y + 6.0;
/// How high above the water its ripples are made, in units.
const RIPPLE_HEIGHT: f32 = 2.0;
/// The scale its ripples start at.
const RIPPLE_SCALE: f32 = 2.0;
/// Seconds between ripples (the `.3f` in `UpdateSkippy`).
const RIPPLE_INTERVAL: f32 = 0.3;

/// How close the player must come for a skippy to leave its spline, in
/// units (`SKIPPY_CHASE_RANGE`).
const CHASE_RANGE: f32 = 500.0;
/// Turn speed, in radians per second (`SKIPPY_TURN_SPEED`).
const TURN_SPEED: f32 = 4.5;
/// Speed along a spline, in baked points per second
/// (`SKIPPY_SPLINE_SPEED`).
const SPLINE_SPEED: f32 = 200.0;
/// The speed a stroke gives, and the most a skippy keeps, in units per
/// second. The original caps it because collisions could otherwise speed
/// it up without limit.
const STROKE_SPEED: f32 = 400.0;
/// How fast a skippy slows down between strokes, in units per second
/// squared.
const SLOWDOWN: f32 = 500.0;

/// `SKIPPY_HEALTH`.
const HEALTH: f32 = 0.5;
/// What touching a skippy does to the player (`SKIPPY_DAMAGE`).
const DAMAGE: f32 = 0.05;
/// `SKIPPY_SCALE`.
const SCALE: f32 = 0.7;
/// The collision box (`SetObjectCollisionBounds(newObj, 30,-60,-50,50,50,-50)`).
const COLLISION_BOX: CollisionBox = CollisionBox::new(30.0, -60.0, -50.0, 50.0, 50.0, -50.0);

/// The animations (`SKIPPY_ANIM_*`).
const ANIM_SWIM: usize = 0;
const ANIM_DEATH: usize = 1;
/// The animation flag the swim sets at each stroke (`SpeedBoost`,
/// `Flag[0]`).
const SPEED_BOOST_FLAG: usize = 0;

/// What a skippy is doing; it follows its animation, as the original
/// indexes its move table with `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkippyState {
    /// Darting at the player (`MoveSkippy_Swimming`), or along its spline.
    #[default]
    Swimming,
    /// Killed: it floats where it is until out of range
    /// (`MoveSkippy_Death`).
    Dying,
}

/// A skippy's own state.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct SkippyBrain {
    pub state: SkippyState,
    /// Seconds since the last ripple (`RippleTimer`).
    pub ripple_timer: f32,
}

impl SkippyBrain {
    /// Counts the ripple timer up by `dt` seconds and returns whether to
    /// make a ripple now. The ripple part of `UpdateSkippy`.
    fn ripple_due(&mut self, dt: f32) -> bool {
        self.ripple_timer += dt;
        if self.ripple_timer > RIPPLE_INTERVAL {
            self.ripple_timer = 0.0;
            true
        } else {
            false
        }
    }
}

/// The skippy's skeleton, as `AddEnemy_Skippy` and `PrimeEnemy_Skippy`
/// set it up.
fn skippy_skeleton(position: Vec2) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::Skippy, SkeletonType::Skippy, position, SCALE)
        .anim(ANIM_SWIM)
        .health(HEALTH)
        .damage(DAMAGE)
        .with_kinds(CollisionKind::Spiked)
        .collision_box(COLLISION_BOX)
}

/// Port of `AddEnemy_Skippy` (original/src/Enemies/Enemy_Skippy.c). It
/// checks only the total: `MAX_SKIPPY` is defined there but never used.
fn add_skippy(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if enemies.counts().total() >= MAX_ENEMIES {
        return false;
    }
    let Some(skippy) = enemies.spawn(skippy_skeleton(spawn.position).from_item(spawn.index)) else {
        return false;
    };
    enemies
        .commands()
        .entity(skippy)
        .insert(SkippyBrain::default());
    true
}

/// Port of `PrimeEnemy_Skippy` (original/src/Enemies/Enemy_Skippy.c),
/// which puts it on the water's surface.
fn prime_skippy(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, SPLINE_SPEED);
    let floor = enemies
        .map()
        .floor_height(spawn.position.x, spawn.position.y);
    let Some(skippy) = enemies.spawn(
        skippy_skeleton(spawn.position)
            .foot_offset(floor - WATER_Y)
            .on_spline(on_spline),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(skippy)
        .insert(SkippyBrain::default());
    true
}

/// One step of swimming: the new heading, speed and horizontal velocity.
///
/// Port of the speed and heading part of `MoveSkippy_Swimming`
/// (original/src/Enemies/Enemy_Skippy.c). `speed` is last tick's
/// (`UpdateEnemy`'s `Speed`); a stroke (`boost`) sets it to
/// [`STROKE_SPEED`] and it then runs down.
fn swim_step(
    yaw: f32,
    speed: f32,
    boost: bool,
    position: Vec2,
    target: Vec2,
    dt: f32,
) -> (f32, f32, Vec2) {
    let mut speed = speed.min(STROKE_SPEED);
    let (yaw, _) = turn_toward(yaw, position, target, TURN_SPEED * dt);
    if boost {
        speed = STROKE_SPEED;
    }
    speed = (speed - SLOWDOWN * dt).max(0.0);
    (yaw, speed, yaw_forward(yaw) * speed)
}

/// Kills a skippy: it leaves its spline, its item never comes back, it
/// becomes plain scenery for collisions and plays its death. Does nothing
/// to a skippy that is dying already, and returns whether it killed it.
///
/// Port of `KillSkippy` (original/src/Enemies/Enemy_Skippy.c), but for the
/// delta, which the caller zeroes.
fn kill_skippy(
    commands: &mut Commands,
    skippy: Entity,
    brain: &mut SkippyBrain,
    animator: Option<Mut<SkeletonAnimator>>,
    shape: CollisionBox,
) -> bool {
    if brain.state == SkippyState::Dying {
        return false;
    }
    let mut entity = commands.entity(skippy);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    // `CType = CTYPE_MISC`, keeping `MakeEnemySkeleton`'s solid sides.
    entity.insert(solid_object(
        vec![shape],
        CollisionKind::Misc,
        SolidSides::ALL,
    ));
    brain.state = SkippyState::Dying;
    if let Some(mut animator) = animator
        && animator.anim != ANIM_DEATH
    {
        animator.set_anim(ANIM_DEATH);
    }
    true
}

/// A skippy whose health ran out dies where it is.
///
/// Port of the `ENEMY_KIND_SKIPPY` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_skippies(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut skippies: Query<(
        &mut SkippyBrain,
        &mut Velocity,
        &EnemyModel,
        &CollisionBoxes,
    )>,
    mut models: Query<&mut SkeletonAnimator, Without<SkippyBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, mut velocity, model, boxes)) = skippies.get_mut(kill.enemy) else {
            continue;
        };
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        if kill_skippy(
            &mut commands,
            kill.enemy,
            &mut brain,
            models.get_mut(model.0).ok(),
            shape,
        ) {
            **velocity = Vec3::ZERO;
        }
    }
}

/// Moves the skippies that aren't on a spline: swimming at the nearest
/// player, or floating dead.
///
/// Port of `MoveSkippy`, `MoveSkippy_Swimming`, `MoveSkippy_Death` and
/// `UpdateSkippy` (original/src/Enemies/Enemy_Skippy.c). Going out of
/// range is the items' `DespawnOutOfRange`.
fn move_skippies(
    mut commands: Commands,
    map: Res<TerrainMap>,
    mut collision: EnemyCollision,
    mut ripples: RippleMaker,
    mut skippies: Query<(EnemyBody, &mut SkippyBrain, &EnemyModel), Without<OnSpline>>,
    mut models: Query<(&mut SkeletonAnimator, &mut AnimationFlags), Without<SkippyBrain>>,
    players: Query<&Transform, (With<Player>, Without<SkippyBrain>)>,
) {
    let dt = collision.dt();
    for (mut body, mut brain, model) in &mut skippies {
        let skippy = body.entity;
        if brain.state == SkippyState::Swimming {
            let shape = body.boxes.0.first().copied().unwrap_or(COLLISION_BOX);
            let boost = models
                .get_mut(model.0)
                .map(|(_, mut flags)| std::mem::take(&mut flags.0[SPEED_BOOST_FLAG]))
                .unwrap_or(false);
            let mut coord = body.transform.translation;
            let target =
                nearest_player(coord, players.iter().map(|t| t.translation)).unwrap_or(coord);
            let (yaw, _, velocity) = swim_step(
                yaw_of(body.transform.rotation),
                body.velocity.length(),
                boost,
                coord.xz(),
                target.xz(),
                dt,
            );
            body.velocity.x = velocity.x;
            body.velocity.z = velocity.y;
            coord.x += velocity.x * dt;
            coord.z += velocity.y * dt;
            coord.y = SKIPPY_Y;
            body.transform.translation = coord;
            body.transform.rotation = Quat::from_rotation_y(yaw);

            let model_entity = model.0;
            let models = &mut models;
            let brain_ref = &mut *brain;
            let contact =
                collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| {
                    // `KillSkippy`'s zeroed delta is overwritten by `gDelta`
                    // when the move ends, so the velocity is left alone.
                    kill_skippy(
                        &mut commands,
                        skippy,
                        brain_ref,
                        models.get_mut(model_entity).ok().map(|(a, _)| a),
                        shape,
                    );
                    false
                });
            if contact.deleted {
                continue;
            }

            // It stays on the water.
            let at = body.transform.translation;
            if map.floor_height(at.x, at.z) > WATER_Y {
                body.transform.translation = **body.previous;
                body.velocity.x = 0.0;
                body.velocity.z = 0.0;
            }
        }

        if brain.ripple_due(dt) {
            let at = body.transform.translation;
            make_ripple(
                &mut commands,
                &mut ripples,
                Vec3::new(at.x, WATER_Y + RIPPLE_HEIGHT, at.z),
                RIPPLE_SCALE,
            );
        }
    }
}

/// Moves the skippies along their splines, and lets them off to chase a
/// player that comes in range.
///
/// Port of `MoveSkippyOnSpline` (original/src/Enemies/Enemy_Skippy.c). The
/// collision box follows the transform by itself.
fn move_skippies_on_spline(
    time: Res<Time>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut skippies: Query<(
        Entity,
        &mut Transform,
        &mut OnSpline,
        &PreviousPosition,
        &SkippyBrain,
    )>,
    players: Query<&Transform, (With<Player>, Without<SkippyBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (skippy, mut transform, mut on_spline, previous, brain) in &mut skippies {
        if brain.state == SkippyState::Dying {
            continue;
        }
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;
        if !on_spline.visible {
            continue;
        }
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), previous.xz(), position);
        transform.rotation = Quat::from_rotation_y(yaw);

        let at = transform.translation;
        let near = nearest_player(at, players.iter().map(|t| t.translation))
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < CHASE_RANGE);
        if near {
            detach_enemy_from_spline(&mut commands.entity(skippy));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_stroke_sets_the_speed_which_then_runs_down() {
        // Yaw 0 faces -Z, where the player is.
        let player = Vec2::new(0.0, -1000.0);
        let (yaw, speed, velocity) = swim_step(0.0, 0.0, true, Vec2::ZERO, player, DT);
        assert_eq!(yaw, 0.0);
        assert!((speed - (STROKE_SPEED - SLOWDOWN * DT)).abs() < 1e-3);
        assert!((velocity.y + speed).abs() < 1e-3 && velocity.x.abs() < 1e-3);

        // Without a stroke it slows down to a stop, never backwards.
        let (_, speed, _) = swim_step(0.0, 100.0, false, Vec2::ZERO, player, 0.1);
        assert!((speed - 50.0).abs() < 1e-3);
        let (_, speed, velocity) = swim_step(0.0, 10.0, false, Vec2::ZERO, player, 0.1);
        assert_eq!(speed, 0.0);
        assert_eq!(velocity, Vec2::ZERO);
    }

    #[test]
    fn the_speed_is_capped_and_the_skippy_turns_at_its_rate() {
        let behind = Vec2::new(0.0, 1000.0);
        let (yaw, speed, _) = swim_step(0.0, 5000.0, false, Vec2::ZERO, behind, DT);
        assert!((speed - (STROKE_SPEED - SLOWDOWN * DT)).abs() < 1e-3);
        let turned = yaw.min(std::f32::consts::TAU - yaw);
        assert!((turned - TURN_SPEED * DT).abs() < 1e-4, "{yaw}");
    }

    #[test]
    fn ripples_come_every_interval() {
        let mut brain = SkippyBrain::default();
        let ripples = (0..60).filter(|_| brain.ripple_due(DT)).count();
        // One per 0.3 s and a frame, over a second.
        assert_eq!(ripples, 3);
    }

    #[test]
    fn killing_a_skippy_turns_it_into_scenery_once() {
        let mut world = World::new();
        let skippy = world.spawn_empty().id();
        let mut brain = SkippyBrain::default();
        let killed = bevy::ecs::system::RunSystemOnce::run_system_once(
            &mut world,
            move |mut commands: Commands| {
                let first = kill_skippy(&mut commands, skippy, &mut brain, None, COLLISION_BOX);
                let again = kill_skippy(&mut commands, skippy, &mut brain, None, COLLISION_BOX);
                (first, again, brain.state)
            },
        );
        assert_eq!(killed.ok(), Some((true, false, SkippyState::Dying)));
        let boxes = world.get::<CollisionBoxes>(skippy).map(|b| b.0.clone());
        assert_eq!(boxes, Some(vec![COLLISION_BOX]));
    }
}
