//! The roach: a big cockroach that walks at the player once it comes near,
//! leaving a trail of gas clouds behind it. The gas drains the ball's
//! time, and a hot particle (fire, a firecracker, another cloud going off)
//! sets it off in a burst of burning sparks. Kicked or hit by a fast ball,
//! the roach falls on its butt.
//!
//! Port of original/src/Enemies/Enemy_Roach.c. Roaches are both map items
//! (`AddEnemy_Roach`) and spline items (`PrimeEnemy_Roach`), under
//! [`kind::ROACH`]; they live in the Night and Ant Hill levels. The gas is
//! in [`gas`].
//!
//! The roach hurts the player through the player's own collision (`HurtMe
//! | HurtNoKnock` with [`Damage`](crate::combat::Damage)). It answers
//! [`EnemyKicked`] and [`BallHitEnemy`] (both knock it on its butt) and
//! [`EnemyKilled`]. It isn't boppable or spiked.

mod gas;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled,
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, apply_friction,
    death_enemy_collision_mask, default_enemy_collision_mask, detach_enemy_from_spline, move_enemy,
    nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionKind};
use crate::combat::Health;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::math::{
    GameRandom, quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of,
};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::skeleton::{SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::terrain::TerrainMap;

pub use gas::{GasCloud, GasPuff};

pub struct RoachPlugin;

impl Plugin for RoachPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::ROACH, add_roach)
            .register_spline_item_kind(kind::ROACH, prime_roach)
            .add_systems(
                FixedUpdate,
                (
                    kick_roaches.in_set(EnemySystems::Kicked),
                    // The player's collision reaches the roaches before
                    // they move, as in the original's frame.
                    (ball_hit_roaches, move_roaches)
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_roaches.in_set(EnemySystems::Killed),
                    move_roaches_on_spline.in_set(SplineSystems::Move),
                ),
            )
            .add_plugins(gas::plugin);
    }
}

/// The most roaches counted at once (`MAX_ROACHS`), unless the item says
/// to add it always.
const MAX_ROACHES: usize = 8;
/// `ROACH_SCALE`
const ROACH_SCALE: f32 = 1.7;
/// How close the player must come, in units, for a standing roach to walk
/// at it (`ROACH_CHASE_DIST`).
const ROACH_CHASE_DIST: f32 = 900.0;
/// How fast a roach turns, in radians per second (`ROACH_TURN_SPEED`).
const ROACH_TURN_SPEED: f32 = 2.4;
/// Walking speed, in units per second (`ROACH_WALK_SPEED`).
const ROACH_WALK_SPEED: f32 = 300.0;
/// The walk animation's speed (`ROACH_WALK_SPEED * .004`).
const ROACH_WALK_ANIM_SPEED: f32 = ROACH_WALK_SPEED * 0.004;
/// The roach's origin is this far below its feet (`ROACH_FOOT_OFFSET`).
const ROACH_FOOT_OFFSET: f32 = 0.0;
/// How fast the ball must go to knock a roach down, in units per second
/// (`ROACH_KNOCKDOWN_SPEED`).
const ROACH_KNOCKDOWN_SPEED: f32 = 1700.0;
const ROACH_HEALTH: f32 = 1.0;
/// What touching a free roach does to the player. One made on a spline
/// does nothing (`Damage = 0` in `PrimeEnemy_Roach`).
const ROACH_DAMAGE: f32 = 0.1;
/// The collision box of a roach from a map item (`AddEnemy_Roach`).
const ROACH_BOX: CollisionBox =
    CollisionBox::new(250.0, ROACH_FOOT_OFFSET, -100.0, 100.0, 100.0, -100.0);
/// The smaller box of a roach made on a spline (`PrimeEnemy_Roach`), which
/// it keeps when it leaves the spline.
const ROACH_SPLINE_BOX: CollisionBox =
    CollisionBox::new(70.0, ROACH_FOOT_OFFSET, -70.0, 70.0, 70.0, -70.0);
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const ROACH_SHADOW_SCALE: f32 = 8.0;
/// Speed along a spline, in baked points per second
/// (`IncreaseSplineIndex(theNode, 60)`).
const ROACH_SPLINE_SPEED: f32 = 60.0;
/// How close the player must be, in units, for a roach to leave gas
/// (`GAS_DIST`).
const GAS_DIST: f32 = 1200.0;

/// How long a roach sits on its butt, in seconds (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// Friction on a roach on its butt on the ground, per frame at 60 fps.
const BUTT_FRICTION_PER_FRAME: f32 = 140.0;
/// Friction on a dead roach on the ground, per frame at 60 fps.
const DEATH_FRICTION_PER_FRAME: f32 = 60.0;
/// How far from the player a dead roach must be, as well as out of view,
/// to be deleted, in units.
const DEAD_DELETE_DIST: f32 = 1000.0;

/// The kick's sideways and upward speed for a roach, in units per second
/// (the `KnockRoachOnButt` call in `DoBugKick`).
const KICK_KNOCK_SPEED: f32 = 300.0;
const KICK_KNOCK_RISE: f32 = 700.0;
/// How much of the ball's velocity a knocked-down roach takes
/// (`BallHitRoach`).
const BALL_KNOCK_SHARE: f32 = 0.8;
/// How fast the ball sends a roach up, on top of its share of the ball's
/// own rise, in units per second.
const BALL_KNOCK_RISE: f32 = 250.0;
/// What the ball does to a roach's health.
const BALL_DAMAGE: f32 = 0.5;
/// How much of its velocity the player keeps after knocking a roach down
/// (`gDelta *= .2` in `KnockRoachOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const WALK_MORPH_RATE: f32 = 8.0;
const STAND_MORPH_RATE: f32 = 3.0;
const ON_BUTT_MORPH_RATE: f32 = 9.0;
const DEATH_MORPH_RATE: f32 = 2.0;

/// What a roach is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state plays the animation of the same
/// name (`ROACH_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoachState {
    #[default]
    Stand,
    Walk,
    OnButt,
    Death,
}

impl RoachState {
    /// The state's animation (`ROACH_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Stand => 0,
            Self::Walk => 1,
            Self::OnButt => 2,
            Self::Death => 3,
        }
    }
}

/// A roach's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct RoachBrain {
    pub state: RoachState,
    /// Seconds left on its butt (`ButtTimer`).
    pub butt_timer: f32,
    /// Seconds since it last left a gas cloud (`GasTimer`).
    pub gas_timer: f32,
}

impl RoachBrain {
    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(
        &mut self,
        animator: Option<&mut SkeletonAnimator>,
        state: RoachState,
        rate: f32,
    ) {
        self.state = state;
        if let Some(animator) = animator {
            animator.morph_to(state.anim(), rate);
        }
    }
}

/// The seconds that must pass between two gas clouds (`GasTimer < .4`).
const GAS_INTERVAL: f32 = 0.4;
/// The most a new interval starts ahead by, in seconds (`RandomFloat()*.1`).
const GAS_INTERVAL_JITTER: f32 = 0.1;

/// Counts `dt` toward the next gas cloud and says whether it is due; if it
/// is, the timer starts over a little ahead.
///
/// Port of the timer in `LeaveGasTrail` (original/src/Enemies/Enemy_Roach.c).
fn gas_due(timer: &mut f32, dt: f32, random: &mut GameRandom) -> bool {
    *timer += dt;
    if *timer < GAS_INTERVAL {
        return false;
    }
    *timer = random.next_f32() * GAS_INTERVAL_JITTER;
    true
}

/// The roach's skeleton as both its Add and Prime routines set it up: not
/// `AutoTarget`, unlike most kickable enemies.
fn roach_skeleton(position: Vec2, collision_box: CollisionBox, damage: f32) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::Roach, SkeletonType::Roach, position, ROACH_SCALE)
        .foot_offset(ROACH_FOOT_OFFSET)
        .collision_box(collision_box)
        .with_kinds([
            CollisionKind::Kickable,
            CollisionKind::HurtMe,
            CollisionKind::HurtNoKnock,
        ])
        .health(ROACH_HEALTH)
        .damage(damage)
        .shadow(ROACH_SHADOW_SCALE)
}

/// Whether a map item may add another roach: always if bit 0 of
/// `params[3]` is set, otherwise while fewer than [`MAX_ROACHES`] are
/// counted. Unlike most kinds, it ignores the total of all enemies.
fn may_add_roach(params: [u8; 4], roaches: usize) -> bool {
    params[3] & 1 != 0 || roaches < MAX_ROACHES
}

/// Port of `AddEnemy_Roach` (original/src/Enemies/Enemy_Roach.c).
fn add_roach(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !may_add_roach(spawn.params, enemies.counts().of_kind(EnemyKind::Roach)) {
        return false;
    }
    let Some(roach) = enemies.spawn(
        roach_skeleton(spawn.position, ROACH_BOX, ROACH_DAMAGE)
            .from_item(spawn.index)
            .anim(RoachState::Stand.anim()),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(roach)
        .insert(RoachBrain::default());
    true
}

/// Port of `PrimeEnemy_Roach` (original/src/Enemies/Enemy_Roach.c): a
/// harmless roach walking its spline. Like every prime routine it doesn't
/// check the enemy counts.
fn prime_roach(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, ROACH_SPLINE_SPEED);
    let Some(roach) = enemies.spawn(
        roach_skeleton(spawn.position, ROACH_SPLINE_BOX, 0.0)
            .on_spline(on_spline)
            .anim(RoachState::Walk.anim()),
    ) else {
        return false;
    };
    enemies.commands().entity(roach).insert(RoachBrain {
        state: RoachState::Walk,
        ..default()
    });
    true
}

/// Kills a roach: it leaves its spline, never comes back, becomes a plain
/// obstacle and plays its death. It never deletes the roach.
///
/// Port of `KillRoach` (original/src/Enemies/Enemy_Roach.c). The original
/// starts the death blend again each time it is called, which a dead roach
/// in hurting particles is every frame, holding it at the start of the
/// blend; here a dead roach ignores further kills.
fn kill_roach(
    commands: &mut Commands,
    roach: Entity,
    brain: &mut RoachBrain,
    animator: Option<&mut SkeletonAnimator>,
) {
    if brain.state == RoachState::Death {
        return;
    }
    let mut entity = commands.entity(roach);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    brain.morph_state(animator, RoachState::Death, DEATH_MORPH_RATE);
}

/// What [`knock_roach_on_butt`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knock {
    /// It was on its butt already, and nothing happened.
    Ignored,
    /// It went down; the player slows down.
    KnockedDown,
}

/// A roach being knocked over, with what the knock changes.
struct KnockedRoach<'a> {
    entity: Entity,
    brain: &'a mut RoachBrain,
    animator: Option<&'a mut SkeletonAnimator>,
    velocity: &'a mut Velocity,
    health: &'a mut Health,
}

/// Knocks a roach on its butt with `velocity` and hurts it by `damage`,
/// killing it if that was the last of its health.
///
/// Port of `KnockRoachOnButt` (original/src/Enemies/Enemy_Roach.c), but
/// for slowing the player down, which the callers do with the player they
/// know.
fn knock_roach_on_butt(
    roach: KnockedRoach,
    commands: &mut Commands,
    velocity: Vec3,
    damage: f32,
) -> Knock {
    let KnockedRoach {
        entity,
        brain,
        mut animator,
        velocity: roach_velocity,
        health,
    } = roach;
    if brain.state == RoachState::OnButt {
        return Knock::Ignored;
    }
    detach_enemy_from_spline(&mut commands.entity(entity));
    **roach_velocity = velocity;
    brain.morph_state(
        animator.as_deref_mut(),
        RoachState::OnButt,
        ON_BUTT_MORPH_RATE,
    );
    brain.butt_timer = BUTT_TIME;
    if health.lose(damage) {
        kill_roach(commands, entity, brain, animator);
    }
    Knock::KnockedDown
}

/// What the kick and ball handlers need of a roach.
type KnockQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut RoachBrain,
        &'static mut Velocity,
        &'static mut Health,
        &'static EnemyModel,
    ),
    Without<Player>,
>;

/// The players' velocities, which knocking a roach down slows.
type PlayerVelocities<'w, 's> =
    Query<'w, 's, &'static mut Velocity, (With<Player>, Without<RoachBrain>)>;

/// The roaches' animators, on their model children.
type RoachAnimators<'w, 's> = Query<'w, 's, &'static mut SkeletonAnimator, Without<RoachBrain>>;

/// The kick knocks a roach on its butt.
///
/// Port of the `SKELETON_TYPE_ROACH` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_roaches(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut roaches: KnockQuery,
    mut animators: RoachAnimators,
    mut players: PlayerVelocities,
) {
    for kick in kicks.read() {
        let Ok((mut brain, mut velocity, mut health, model)) = roaches.get_mut(kick.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        let roach = KnockedRoach {
            entity: kick.enemy,
            brain: &mut brain,
            animator: animator.as_deref_mut(),
            velocity: &mut velocity,
            health: &mut health,
        };
        let knock = knock_roach_on_butt(
            roach,
            &mut commands,
            kick.knock(KICK_KNOCK_SPEED, KICK_KNOCK_RISE),
            kick.damage,
        );
        if knock == Knock::KnockedDown
            && let Ok(mut player) = players.get_mut(kick.player)
        {
            **player *= PLAYER_SLOWDOWN;
        }
    }
}

/// A fast enough ball knocks a roach on its butt.
///
/// Port of `BallHitRoach` (original/src/Enemies/Enemy_Roach.c).
fn ball_hit_roaches(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut roaches: KnockQuery,
    mut animators: RoachAnimators,
    mut players: PlayerVelocities,
) {
    for hit in hits.read() {
        if hit.ball_speed <= ROACH_KNOCKDOWN_SPEED {
            continue;
        }
        let Ok((mut brain, mut velocity, mut health, model)) = roaches.get_mut(hit.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        let roach = KnockedRoach {
            entity: hit.enemy,
            brain: &mut brain,
            animator: animator.as_deref_mut(),
            velocity: &mut velocity,
            health: &mut health,
        };
        let knock = knock_roach_on_butt(
            roach,
            &mut commands,
            ball_knock(hit.ball_velocity),
            BALL_DAMAGE,
        );
        if knock == Knock::KnockedDown {
            // Sound: EFFECT_POUND at the roach, pitch kMiddleC+2, volume 2.0.
            if let Ok(mut player) = players.get_mut(hit.player) {
                **player *= PLAYER_SLOWDOWN;
            }
        }
    }
}

/// The velocity a ball going at `ball_velocity` knocks a roach over with.
fn ball_knock(ball_velocity: Vec3) -> Vec3 {
    ball_velocity * BALL_KNOCK_SHARE + Vec3::Y * BALL_KNOCK_RISE
}

/// A roach whose health ran out dies.
///
/// Port of the `ENEMY_KIND_ROACH` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_roaches(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut roaches: Query<(&mut RoachBrain, &EnemyModel)>,
    mut animators: RoachAnimators,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = roaches.get_mut(kill.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        kill_roach(
            &mut commands,
            kill.enemy,
            &mut brain,
            animator.as_deref_mut(),
        );
    }
}

/// Moves the roaches that aren't on a spline, by their state, and leaves
/// gas behind the walking ones.
///
/// Port of `MoveRoach`, `MoveRoach_Standing`, `MoveRoach_Walking`,
/// `MoveRoach_OnButt`, `MoveRoach_Death` and `UpdateRoach`
/// (original/src/Enemies/Enemy_Roach.c). Leaving the item window
/// (`TrackTerrainItem`) is `DespawnOutOfRange`. Each roach goes for the
/// nearest player.
#[allow(clippy::too_many_arguments)]
fn move_roaches(
    mut commands: Commands,
    mut collision: EnemyCollision,
    mut random: ResMut<GameRandom>,
    mut gas: MessageWriter<GasPuff>,
    mut roaches: Query<
        (EnemyBody, &mut RoachBrain, &EnemyModel),
        (Without<OnSpline>, Without<Player>),
    >,
    mut animators: RoachAnimators,
    players: Query<&Transform, (With<Player>, Without<RoachBrain>)>,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut brain, model) in &mut roaches {
        let roach = body.entity;
        let Ok(mut animator) = animators.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let player = nearest_player(coord, player_positions.iter().copied()).unwrap_or(coord);
        let state = brain.state;
        let mut mask = default_enemy_collision_mask();

        match state {
            RoachState::Stand => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                if quick_distance(player.xz(), coord.xz()) < ROACH_CHASE_DIST {
                    brain.morph_state(Some(&mut animator), RoachState::Walk, WALK_MORPH_RATE);
                }
                fall(&mut body, 0.0, dt);
            }
            RoachState::Walk => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * ROACH_WALK_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                fall(&mut body, 0.0, dt);
                animator.speed = ROACH_WALK_ANIM_SPEED;
            }
            RoachState::OnButt => {
                let friction = if body.ground.on_ground {
                    per_frame_friction(BUTT_FRICTION_PER_FRAME)
                } else {
                    0.0
                };
                fall(&mut body, friction, dt);
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    brain.morph_state(Some(&mut animator), RoachState::Stand, STAND_MORPH_RATE);
                }
            }
            RoachState::Death => {
                if culling.is_culled(coord, **body.radius)
                    && quick_distance(coord.xz(), player.xz()) > DEAD_DELETE_DIST
                {
                    commands.entity(roach).despawn();
                    continue;
                }
                let friction = if body.ground.on_ground {
                    per_frame_friction(DEATH_FRICTION_PER_FRAME)
                } else {
                    0.0
                };
                fall(&mut body, friction, dt);
                mask = death_enemy_collision_mask();
            }
        }

        // Port of `DoEnemyCollisionDetect`. `KillRoach` never deletes, so
        // its effects can wait until the collision ends.
        let mut killed = false;
        let contact = collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        // Only a standing or walking roach drowns.
        let drowns =
            matches!(state, RoachState::Stand | RoachState::Walk) && contact.underwater.is_some();
        if killed || drowns {
            kill_roach(&mut commands, roach, &mut brain, Some(&mut animator));
        }

        // `UpdateRoach(theNode, true)`, from the walk only.
        let at = body.transform.translation;
        if state == RoachState::Walk
            && quick_distance(at.xz(), player.xz()) < GAS_DIST
            && gas_due(&mut brain.gas_timer, dt, &mut random)
        {
            gas.write(GasPuff { at });
        }
    }
}

/// Turns toward `target` at [`ROACH_TURN_SPEED`] for `dt` seconds
/// (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) {
    let (yaw, _) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        ROACH_TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
}

/// Slows by `friction` (units per second squared), falls under gravity and
/// moves, for `dt` seconds.
fn fall(body: &mut super::EnemyBodyItem, friction: f32, dt: f32) {
    let mut velocity = **body.velocity;
    apply_friction(&mut velocity, friction, dt);
    velocity.y -= ENEMY_GRAVITY * dt;
    **body.velocity = velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// Moves the roaches along their splines. Those the player can see face
/// where they go, stand on the floor, leave gas near the player and can be
/// hurt by explosions.
///
/// Port of `MoveRoachOnSpline` (original/src/Enemies/Enemy_Roach.c). The
/// collision box and shadow follow the transform by themselves. The
/// original runs the collision on whatever object moved last (its
/// `gCoord` isn't set here), which can drag nothing but the hurt along;
/// here it is the roach's own, and the roach stays on its spline.
#[allow(clippy::too_many_arguments)]
fn move_roaches_on_spline(
    mut commands: Commands,
    mut collision: EnemyCollision,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut random: ResMut<GameRandom>,
    mut gas: MessageWriter<GasPuff>,
    mut roaches: Query<(EnemyBody, &mut OnSpline, &mut RoachBrain, &EnemyModel), Without<Player>>,
    mut animators: RoachAnimators,
    players: Query<&Transform, (With<Player>, Without<RoachBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut on_spline, mut brain, model) in &mut roaches {
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        body.transform.translation.x = position.x;
        body.transform.translation.z = position.y;
        if !on_spline.visible {
            continue;
        }
        let previous: &PreviousPosition = body.previous;
        let yaw = yaw_from_point_to_point(yaw_of(body.transform.rotation), previous.xz(), position);
        body.transform.rotation = Quat::from_rotation_y(yaw);
        body.transform.translation.y = map.floor_height(position.x, position.y) - ROACH_FOOT_OFFSET;

        let at = body.transform.translation;
        let near = nearest_player(at, player_positions.iter().copied())
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < GAS_DIST);
        if near && gas_due(&mut brain.gas_timer, dt, &mut random) {
            gas.write(GasPuff { at });
        }

        // "Just do this to see if explosions hurt."
        let mut killed = false;
        collision.collide(&mut body, CollisionKind::HurtEnemy.into(), &mut |_, _| {
            killed = true;
            false
        });
        body.transform.translation = at;
        if killed {
            let entity = body.entity;
            let mut animator = animators.get_mut(model.0).ok();
            kill_roach(&mut commands, entity, &mut brain, animator.as_deref_mut());
        }
    }
}

#[cfg(test)]
mod tests;
