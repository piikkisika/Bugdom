//! The fire ant: a winged ant with a flame on its head. It stands and
//! turns to face the player, breathes fire at a player in range, and takes
//! off to fly at a player who comes close, breathing fire all the way.
//! Knocked down by the ball, it falls on its butt; killed, it falls and
//! dies.
//!
//! Port of original/src/Enemies/Enemy_FireAnt.c. Fire ants are map items
//! (`AddEnemy_FireAnt`) under [`kind::FIRE_ANT`]. The breath is a group of
//! hurting fire particles ([`ParticleFlags::HURT_PLAYER`]), which the
//! particle simulation tests against the player
//! (`MoveParticleGroups`).
//!
//! Not ported, because the original never reaches it:
//! - `PrimeEnemy_FireAnt` and `MoveFireAntOnSpline`. The spline item table
//!   (`gSplineItemPrimeRoutines`, original/src/Terrain/SplineItems.c) has
//!   `NilPrime` for the fire ant's kind, and its "Fireant enemy" entry
//!   primes an ordinary ant, so no fire ant is ever on a spline.
//! - The kick: fire ants are kickable, but `DoBugKick` has no case for
//!   them, so a kick does nothing to one. This plugin doesn't answer
//!   [`EnemyKicked`](super::EnemyKicked) either.
//!
//! The walk ([`FireAntState::Walk`]) is ported, though only the unused
//! spline prime starts it.

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyBodyItem, EnemyCollision, EnemyCulling,
    EnemyKilled, EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, MAX_ENEMIES,
    apply_friction, death_enemy_collision_mask, default_enemy_collision_mask, move_enemy,
    nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionKind, SolidSides};
use crate::combat::Health;
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture,
};
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_forward, yaw_of};
use crate::physics::Velocity;
use crate::player::Player;
use crate::skeleton::{SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::terrain::TerrainMap;

pub struct FireAntPlugin;

impl Plugin for FireAntPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::FIRE_ANT, add_fire_ant)
            .add_systems(
                FixedUpdate,
                (
                    // The player's collision reaches the fire ants before
                    // they move, as in the original's frame.
                    (ball_hit_fire_ants, move_fire_ants, emit_fire_ant_flames)
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_fire_ants.in_set(EnemySystems::Killed),
                ),
            );
    }
}

/// `FIREANT_SCALE`
const FIRE_ANT_SCALE: f32 = 1.2;
const FIRE_ANT_HEALTH: f32 = 1.0;
/// What touching a fire ant does to the player (`FIREANT_DAMAGE`).
const FIRE_ANT_DAMAGE: f32 = 0.2;
/// The fire ant's origin is this far below its feet (`FIREANT_FOOT_OFFSET`);
/// being negative, it lifts the fire ant.
const FIRE_ANT_FOOT_OFFSET: f32 = -130.0;
/// The top of the collision box, in units above the origin.
const FIRE_ANT_HEAD_OFFSET: f32 = 70.0;
/// Half the collision box's width and depth, in units.
const FIRE_ANT_HALF_WIDTH: f32 = 70.0;
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const FIRE_ANT_SHADOW_SCALE: f32 = 8.0;

/// How close the player must come, in units, for a fire ant to take off
/// (`FIREANT_START_FLY_DIST`).
const START_FLY_DIST: f32 = 500.0;
/// How far the player must get, in units, for a flying fire ant to land
/// (`FIREANT_END_FLY_DIST`).
const END_FLY_DIST: f32 = 1000.0;
/// How close the player must be, in units, for a standing fire ant to
/// breathe fire (`FIREANT_ATTACK_DIST`).
const ATTACK_DIST: f32 = 900.0;
/// How well aimed at the player a fire ant must be to breathe fire, in
/// radians.
const ATTACK_MIN_ANGLE: f32 = 0.03;
/// How fast a fire ant turns, in radians per second (`FIREANT_TURN_SPEED`).
const TURN_SPEED: f32 = 2.4;
/// Walking speed, in units per second (`FIREANT_WALK_SPEED`).
const WALK_SPEED: f32 = 400.0;
/// The walk animation's speed (`FIREANT_WALK_SPEED * .0032`).
const WALK_ANIM_SPEED: f32 = WALK_SPEED * 0.0032;
/// Flying speed, in units per second (`FIREANT_FLY_SPEED`).
const FLY_SPEED: f32 = 200.0;
/// How high above the floor a fire ant flies, in units
/// (`FIREANT_FLY_HEIGHT`).
const FLY_HEIGHT: f32 = 320.0;
/// How fast a flying fire ant speeds up toward its flying height, in units
/// per second squared.
const FLY_CLIMB_ACCEL: f32 = 1000.0;
/// How long a fire ant stands breathing fire, in seconds
/// (`TIME_TO_BREATH`).
const BREATH_TIME: f32 = 4.0;

/// How fast the ball must go to knock a fire ant down, in units per second
/// (`FIREANT_KNOCKDOWN_SPEED`).
const KNOCKDOWN_SPEED: f32 = 1400.0;
/// How much of the ball's velocity a knocked-down fire ant takes
/// (`BallHitFireAnt`).
const BALL_KNOCK_SHARE: f32 = 0.8;
/// How fast the ball sends a fire ant up, on top of its share of the
/// ball's own rise, in units per second.
const BALL_KNOCK_RISE: f32 = 250.0;
/// What the ball does to a fire ant's health (`EnemyGotHurt(enemy, .5)`).
const BALL_DAMAGE: f32 = 0.5;
/// How much of its velocity the player keeps after knocking a fire ant
/// down (`gDelta *= .2` in `KnockFireAntOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;

/// How long a fire ant sits on its butt, in seconds (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// Friction on a fire ant on its butt on the ground, per frame at 60 fps.
const BUTT_FRICTION_PER_FRAME: f32 = 140.0;
/// Friction while getting up and while dead, per frame at 60 fps.
const GET_UP_FRICTION_PER_FRAME: f32 = 60.0;
/// How far from the player a dead fire ant must be, as well as out of
/// view, to be deleted, in units.
const DEAD_DELETE_DIST: f32 = 600.0;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const FLY_MORPH_RATE: f32 = 7.0;
const BREATHE_MORPH_RATE: f32 = 8.0;
const STAND_FROM_BREATHE_MORPH_RATE: f32 = 4.0;
const LAND_MORPH_RATE: f32 = 7.0;
const FALL_ON_BUTT_MORPH_RATE: f32 = 9.0;

/// The head's joint (`FIREANT_HEAD_LIMB`).
const HEAD_JOINT: usize = 1;

/// Where on the head the flame burns, in the head joint's space.
const HEAD_FLAME_OFFSET: Vec3 = Vec3::new(0.0, 30.0, -10.0);
/// Seconds between the head flame's particles.
const HEAD_FLAME_INTERVAL: f32 = 0.01;
/// How far each flame starts from the flame's spot, in units (the whole
/// width).
const HEAD_FLAME_SPREAD: Vec3 = Vec3::new(50.0, 30.0, 50.0);
/// The flames' largest speed in each direction, in units per second (the
/// whole width), and the speed they rise at on average.
const HEAD_FLAME_SPEED: Vec3 = Vec3::new(50.0, 30.0, 50.0);
const HEAD_FLAME_RISE: f32 = 40.0;
/// The smallest flame's scale; they are up to 1 bigger.
const HEAD_FLAME_MIN_SCALE: f32 = 1.7;
/// The head flame (`UpdateFireAnt`).
const HEAD_FLAME_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Gravitoids,
    flags: ParticleFlags::HOT,
    gravity: 0.0,
    magnetism: 10000.0,
    base_scale: 15.0,
    decay_rate: 1.0,
    fade_rate: 0.0,
    texture: ParticleTexture::Fire,
};

/// The mouth, and the middle of the head, in the head joint's space. The
/// fire goes from the one through the other.
const MOUTH_OFFSET: Vec3 = Vec3::new(0.0, -8.0, -25.0);
const HEAD_CENTER_OFFSET: Vec3 = Vec3::new(0.0, 10.0, 0.0);
/// Seconds between bursts of breath (`BreathRegulator`).
const BREATH_INTERVAL: f32 = 0.02;
/// Flames per burst.
const BREATH_PARTICLES: usize = 4;
/// The flames' slowest speed, in units per second; they are up to
/// [`BREATH_EXTRA_SPEED`] faster.
const BREATH_MIN_SPEED: f32 = 400.0;
const BREATH_EXTRA_SPEED: f32 = 200.0;
/// The largest angle, in radians, by which each flame is turned about each
/// axis, to spread them in a cone.
const BREATH_SPREAD: f32 = 0.15;
const BREATH_SCALE: f32 = 1.5;
/// The fire breath (`FireAntBreathFire`): it hurts the player it touches.
const BREATH_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(
        ParticleFlags::BOUNCE.0 | ParticleFlags::HURT_PLAYER.0 | ParticleFlags::HOT.0,
    ),
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 15.0,
    decay_rate: -1.4,
    fade_rate: 1.1,
    texture: ParticleTexture::Fire,
};

/// How many sparks of each colour the ball knocks out of a fire ant.
const BALL_SPARKS: usize = 50;
/// The white sparks' largest speed in each direction, in units per second
/// (the whole width).
const BALL_WHITE_SPARK_SPEED: f32 = 1000.0;
/// The fire sparks' largest speed in each direction (the whole width),
/// and how much of it is upward.
const BALL_FIRE_SPARK_SPEED: f32 = 500.0;
const BALL_FIRE_SPARK_RISE_SHARE: f32 = 0.1;
/// The smallest spark's scale; they are up to 1 bigger.
const BALL_SPARK_MIN_SCALE: f32 = 1.0;
/// The white sparks of a fire ant the ball hits (`BallHitFireAnt`).
const BALL_WHITE_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::HOT.0),
    gravity: 500.0,
    magnetism: 0.0,
    base_scale: 20.0,
    decay_rate: 0.9,
    fade_rate: 0.0,
    texture: ParticleTexture::White,
};
/// Its fire sparks.
const BALL_FIRE_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    decay_rate: 0.7,
    texture: ParticleTexture::Fire,
    ..BALL_WHITE_SPARK_GROUP
};

/// What a fire ant is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state plays the animation of the same
/// name (`FIREANT_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FireAntState {
    #[default]
    Stand,
    Walk,
    /// Standing, breathing fire (`FIREANT_ANIM_STANDFIRE`).
    BreatheFire,
    Fly,
    FallOnButt,
    GetOffButt,
    Die,
}

impl FireAntState {
    /// The state's animation (`FIREANT_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Stand => 0,
            Self::Walk => 1,
            Self::BreatheFire => 2,
            Self::Fly => 3,
            Self::FallOnButt => 4,
            Self::GetOffButt => 5,
            Self::Die => 6,
        }
    }
}

/// A fire ant's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct FireAntBrain {
    pub state: FireAntState,
    /// Dies once it is off its butt (`Dying`).
    pub dying: bool,
    /// Seconds left on its butt (`ButtTimer`).
    pub butt_timer: f32,
    /// Seconds it has been breathing fire (`BreathTimer`).
    pub breath_timer: f32,
}

impl FireAntBrain {
    /// Switches to `state` and starts its animation (`SetSkeletonAnim`).
    fn set_state(&mut self, animator: &mut SkeletonAnimator, state: FireAntState) {
        self.state = state;
        animator.set_anim(state.anim());
    }

    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(&mut self, animator: &mut SkeletonAnimator, state: FireAntState, rate: f32) {
        self.state = state;
        animator.morph_to(state.anim(), rate);
    }
}

/// A fire ant's flames: what its move asked for this tick, and the timers
/// and particle groups of its head flame and its breath.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct FireAntFlames {
    /// The head flame burns this tick (`UpdateFireAnt`'s `updateFlame`).
    pub head_flame: bool,
    /// It breathes fire this tick (`FireAntBreathFire`).
    pub breathe: bool,
    /// Seconds since the last head flame particle (`FireTimer`).
    head_timer: f32,
    /// The head flame's group (`ParticleGroup`).
    head_group: Option<ParticleGroupId>,
    /// Seconds since the last burst of breath (`BreathRegulator`).
    breath_regulator: f32,
    /// The breath's group (`BreathParticleGroup`).
    breath_group: Option<ParticleGroupId>,
}

impl FireAntFlames {
    /// Starts a new breath, in a new group (when the fire ant starts
    /// `FIREANT_ANIM_STANDFIRE`).
    fn start_breath(&mut self) {
        self.breath_regulator = 0.0;
        self.breath_group = None;
    }

    /// Whether a burst of breath is due after `dt` more seconds.
    fn breath_due(&mut self, dt: f32) -> bool {
        self.breath_regulator += dt;
        if self.breath_regulator < BREATH_INTERVAL {
            return false;
        }
        self.breath_regulator = 0.0;
        true
    }

    /// Whether a head flame particle is due after `dt` more seconds.
    fn head_flame_due(&mut self, dt: f32) -> bool {
        self.head_timer += dt;
        if self.head_timer <= HEAD_FLAME_INTERVAL {
            return false;
        }
        self.head_timer = 0.0;
        true
    }
}

/// Port of `AddEnemy_FireAnt` (original/src/Enemies/Enemy_FireAnt.c). Bit 0
/// of `params[3]` adds it however many there are. Otherwise only the count
/// of fire ants is checked, against [`MAX_ENEMIES`] and with `>`, as in the
/// original (whose check of the total is commented out).
fn add_fire_ant(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let always_add = spawn.params[3] & 1 != 0;
    if !always_add && enemies.counts().of_kind(EnemyKind::FireAnt) > MAX_ENEMIES {
        return false;
    }
    let Some(fire_ant) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::FireAnt,
            SkeletonType::FireAnt,
            spawn.position,
            FIRE_ANT_SCALE,
        )
        .from_item(spawn.index)
        .foot_offset(FIRE_ANT_FOOT_OFFSET)
        .collision_box(CollisionBox::new(
            FIRE_ANT_HEAD_OFFSET,
            FIRE_ANT_FOOT_OFFSET,
            -FIRE_ANT_HALF_WIDTH,
            FIRE_ANT_HALF_WIDTH,
            FIRE_ANT_HALF_WIDTH,
            -FIRE_ANT_HALF_WIDTH,
        ))
        .kickable()
        .solid(SolidSides::NOT_TOP)
        .health(FIRE_ANT_HEALTH)
        .damage(FIRE_ANT_DAMAGE)
        .anim(FireAntState::Stand.anim())
        .shadow(FIRE_ANT_SHADOW_SCALE),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(fire_ant)
        .insert((FireAntBrain::default(), FireAntFlames::default()));
    true
}

/// Kills a fire ant: it never comes back, becomes a plain obstacle, and
/// dies once it is off its butt. Repeats are ignored.
///
/// Port of `KillFireAnt` (original/src/Enemies/Enemy_FireAnt.c). There is
/// no spline to leave (see the module docs).
fn kill_fire_ant(
    commands: &mut Commands,
    fire_ant: Entity,
    brain: &mut FireAntBrain,
    animator: Option<&mut SkeletonAnimator>,
) {
    if brain.dying {
        return;
    }
    let mut entity = commands.entity(fire_ant);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    brain.dying = true;
    if brain.state != FireAntState::FallOnButt {
        match animator {
            Some(animator) => brain.set_state(animator, FireAntState::FallOnButt),
            None => brain.state = FireAntState::FallOnButt,
        }
        brain.butt_timer = BUTT_TIME;
    }
}

/// Knocks a fire ant on its butt with `velocity` and hurts it by `damage`,
/// killing it if that was the last of its health. Returns false if it was
/// on its butt already, and nothing happened.
///
/// Port of `KnockFireAntOnButt` (original/src/Enemies/Enemy_FireAnt.c), but
/// for slowing the player down, which the caller does with the player it
/// knows.
#[allow(clippy::too_many_arguments)]
fn knock_fire_ant_on_butt(
    commands: &mut Commands,
    fire_ant: Entity,
    brain: &mut FireAntBrain,
    mut animator: Option<&mut SkeletonAnimator>,
    fire_ant_velocity: &mut Velocity,
    health: &mut Health,
    velocity: Vec3,
    damage: f32,
) -> bool {
    if brain.state == FireAntState::FallOnButt {
        return false;
    }
    **fire_ant_velocity = velocity;
    match animator.as_deref_mut() {
        Some(animator) => {
            brain.morph_state(animator, FireAntState::FallOnButt, FALL_ON_BUTT_MORPH_RATE);
        }
        None => brain.state = FireAntState::FallOnButt,
    }
    brain.butt_timer = BUTT_TIME;
    if health.lose(damage) {
        kill_fire_ant(commands, fire_ant, brain, animator);
    }
    true
}

/// A fast enough ball knocks sparks out of a fire ant and knocks it on its
/// butt.
///
/// Port of `BallHitFireAnt` (original/src/Enemies/Enemy_FireAnt.c).
#[allow(clippy::too_many_arguments)]
fn ball_hit_fire_ants(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut fire_ants: Query<
        (
            &Transform,
            &mut FireAntBrain,
            &mut Velocity,
            &mut Health,
            &EnemyModel,
        ),
        Without<Player>,
    >,
    mut animators: Query<&mut SkeletonAnimator, Without<FireAntBrain>>,
    mut players: Query<&mut Velocity, (With<Player>, Without<FireAntBrain>)>,
) {
    for hit in hits.read() {
        if hit.ball_speed <= KNOCKDOWN_SPEED {
            continue;
        }
        let Ok((transform, mut brain, mut velocity, mut health, model)) =
            fire_ants.get_mut(hit.enemy)
        else {
            continue;
        };
        ball_sparks(&mut groups, &mut random, transform.translation);
        let knocked = knock_fire_ant_on_butt(
            &mut commands,
            hit.enemy,
            &mut brain,
            animators.get_mut(model.0).ok().as_deref_mut(),
            &mut velocity,
            &mut health,
            ball_knock(hit.ball_velocity),
            BALL_DAMAGE,
        );
        if knocked && let Ok(mut player) = players.get_mut(hit.player) {
            **player *= PLAYER_SLOWDOWN;
        }
        // Sound: EFFECT_POUND at the player, pitch kMiddleC+2, volume 2.0.
    }
}

/// The velocity a ball going at `ball_velocity` knocks a fire ant over
/// with.
fn ball_knock(ball_velocity: Vec3) -> Vec3 {
    ball_velocity * BALL_KNOCK_SHARE + Vec3::Y * BALL_KNOCK_RISE
}

/// The white and fire sparks the ball knocks out of a fire ant at `at`
/// (`BallHitFireAnt`).
fn ball_sparks(groups: &mut ParticleGroups, random: &mut GameRandom, at: Vec3) {
    if let Some(group) = groups.new_group(BALL_WHITE_SPARK_GROUP) {
        for _ in 0..BALL_SPARKS {
            let mut spread = || (random.next_f32() - 0.5) * BALL_WHITE_SPARK_SPEED;
            let velocity = Vec3::new(spread(), spread(), spread());
            let scale = random.next_f32() + BALL_SPARK_MIN_SCALE;
            groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
        }
    }
    if let Some(group) = groups.new_group(BALL_FIRE_SPARK_GROUP) {
        for _ in 0..BALL_SPARKS {
            let x = (random.next_f32() - 0.5) * BALL_FIRE_SPARK_SPEED;
            let y = (random.next_f32() - 0.5 + BALL_FIRE_SPARK_RISE_SHARE) * BALL_FIRE_SPARK_SPEED;
            let z = (random.next_f32() - 0.5) * BALL_FIRE_SPARK_SPEED;
            let scale = random.next_f32() + BALL_SPARK_MIN_SCALE;
            groups.add_particle(group, at, Vec3::new(x, y, z), scale, FULL_ALPHA);
        }
    }
}

/// A fire ant whose health ran out dies.
///
/// Port of the `ENEMY_KIND_FIREANT` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_fire_ants(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut fire_ants: Query<(&mut FireAntBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator, Without<FireAntBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = fire_ants.get_mut(kill.enemy) else {
            continue;
        };
        kill_fire_ant(
            &mut commands,
            kill.enemy,
            &mut brain,
            animators.get_mut(model.0).ok().as_deref_mut(),
        );
    }
}

/// What a standing fire ant, `aim` radians off the player and `distance`
/// units from it, starts doing, if anything (`MoveFireAnt_Standing`): it
/// takes off when the player is close, and otherwise breathes fire once it
/// faces a player in range.
fn standing_decision(aim: f32, distance: f32) -> Option<FireAntState> {
    if distance < START_FLY_DIST {
        Some(FireAntState::Fly)
    } else if aim < ATTACK_MIN_ANGLE && distance < ATTACK_DIST {
        Some(FireAntState::BreatheFire)
    } else {
        None
    }
}

/// Moves a flying fire ant's height `y` and climb speed `vy` toward
/// `target` for `dt` seconds, stopping on it (`MoveFireAnt_Fly`).
fn hover(y: &mut f32, vy: &mut f32, target: f32, dt: f32) {
    if *y < target {
        *vy += FLY_CLIMB_ACCEL * dt;
        *y += *vy * dt;
        if *y >= target {
            *y = target;
            *vy = 0.0;
        }
    } else if *y > target {
        *vy -= FLY_CLIMB_ACCEL * dt;
        *y += *vy * dt;
        if *y <= target {
            *y = target;
            *vy = 0.0;
        }
    } else {
        *vy = 0.0;
    }
}

/// Turns toward `target` at [`TURN_SPEED`] for `dt` seconds and returns the
/// angle still to turn (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) -> f32 {
    let (yaw, aim) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
    aim
}

/// Falls under gravity and moves (`gDelta.y -= ENEMY_GRAVITY`,
/// `MoveEnemy`).
fn fall(body: &mut EnemyBodyItem, dt: f32) {
    body.velocity.y -= ENEMY_GRAVITY * dt;
    let velocity = **body.velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// Falls under gravity, slowed by `friction` (per frame at 60 fps) if
/// `slow`: the move of a fire ant on its butt, getting up or dead.
fn tumble(body: &mut EnemyBodyItem, friction: f32, slow: bool, dt: f32) {
    if slow {
        apply_friction(&mut body.velocity, per_frame_friction(friction), dt);
    }
    fall(body, dt);
}

/// Moves the fire ants by their state.
///
/// Port of `MoveFireAnt`, `MoveFireAnt_Standing`,
/// `MoveFireAnt_StandBreathFire`, `MoveFireAnt_Walking`, `MoveFireAnt_Fly`,
/// `MoveFireAnt_FallOnButt`, `MoveFireAnt_GetOffButt`, `MoveFireAnt_Death`
/// and `UpdateFireAnt` (original/src/Enemies/Enemy_FireAnt.c). Leaving the
/// item window (`TrackTerrainItem`) is `DespawnOutOfRange`. The flames the
/// move asks for are made by [`emit_fire_ant_flames`] right after it. Each
/// fire ant goes for the nearest player.
fn move_fire_ants(
    mut commands: Commands,
    mut collision: EnemyCollision,
    map: Res<TerrainMap>,
    mut fire_ants: Query<
        (
            EnemyBody,
            &mut FireAntBrain,
            &mut FireAntFlames,
            &EnemyModel,
        ),
        Without<Player>,
    >,
    mut animators: Query<&mut SkeletonAnimator, Without<FireAntBrain>>,
    players: Query<&Transform, (With<Player>, Without<FireAntBrain>)>,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut brain, mut flames, model) in &mut fire_ants {
        let fire_ant = body.entity;
        let Ok(mut animator) = animators.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let player = nearest_player(coord, player_positions.iter().copied()).unwrap_or(coord);
        flames.breathe = false;
        flames.head_flame = false;
        let mut mask = default_enemy_collision_mask();

        // What follows the collision goes by the state the move started
        // in, as the original's move routines carry on after it.
        let state = brain.state;
        match state {
            FireAntState::Stand => {
                let aim = turn_toward_target(&mut body.transform, player.xz(), dt);
                let distance = quick_distance(player.xz(), coord.xz());
                match standing_decision(aim, distance) {
                    Some(FireAntState::Fly) => {
                        brain.morph_state(&mut animator, FireAntState::Fly, FLY_MORPH_RATE);
                        body.velocity.y = 0.0;
                    }
                    Some(FireAntState::BreatheFire) => {
                        brain.morph_state(
                            &mut animator,
                            FireAntState::BreatheFire,
                            BREATHE_MORPH_RATE,
                        );
                        brain.breath_timer = 0.0;
                        flames.start_breath();
                    }
                    _ => {}
                }
                fall(&mut body, dt);
            }
            FireAntState::BreatheFire => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                flames.breathe = true;
                brain.breath_timer += dt;
                if brain.breath_timer > BREATH_TIME {
                    brain.morph_state(
                        &mut animator,
                        FireAntState::Stand,
                        STAND_FROM_BREATHE_MORPH_RATE,
                    );
                }
                fall(&mut body, dt);
            }
            FireAntState::Walk => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * WALK_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                fall(&mut body, dt);
                animator.speed = WALK_ANIM_SPEED;
            }
            FireAntState::Fly => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * FLY_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                let mut at = body.transform.translation;
                at.x += forward.x * dt;
                at.z += forward.y * dt;
                let target = map.floor_height(at.x, at.z) + FLY_HEIGHT;
                hover(&mut at.y, &mut body.velocity.y, target, dt);
                body.transform.translation = at;
                flames.breathe = true;
            }
            FireAntState::FallOnButt => {
                let on_ground = body.ground.on_ground;
                tumble(&mut body, BUTT_FRICTION_PER_FRAME, on_ground, dt);
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    if brain.dying {
                        brain.set_state(&mut animator, FireAntState::Die);
                        // Straight to `UpdateFireAnt`, without the collision.
                        continue;
                    }
                    brain.set_state(&mut animator, FireAntState::GetOffButt);
                }
            }
            FireAntState::GetOffButt => {
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, true, dt);
                if animator.has_stopped {
                    brain.set_state(&mut animator, FireAntState::Stand);
                }
            }
            FireAntState::Die => {
                let at = body.transform.translation;
                if culling.is_culled(at, **body.radius)
                    && quick_distance(at.xz(), player.xz()) > DEAD_DELETE_DIST
                {
                    commands.entity(fire_ant).despawn();
                    continue;
                }
                let on_ground = body.ground.on_ground;
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, on_ground, dt);
                mask = death_enemy_collision_mask();
            }
        }

        // Port of `DoEnemyCollisionDetect`. `KillFireAnt` never deletes,
        // so its effects can wait until the collision ends.
        let mut killed = false;
        collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        if killed {
            kill_fire_ant(&mut commands, fire_ant, &mut brain, Some(&mut *animator));
        }

        match state {
            FireAntState::BreatheFire => {
                let at = body.transform.translation;
                if quick_distance(player.xz(), at.xz()) < START_FLY_DIST {
                    brain.morph_state(&mut animator, FireAntState::Fly, FLY_MORPH_RATE);
                    body.velocity.y = 0.0;
                }
            }
            FireAntState::Fly => {
                let at = body.transform.translation;
                if quick_distance(player.xz(), at.xz()) > END_FLY_DIST {
                    brain.morph_state(&mut animator, FireAntState::Stand, LAND_MORPH_RATE);
                    **body.velocity = Vec3::ZERO;
                }
            }
            _ => {}
        }
        // `UpdateFireAnt(theNode, true)` from these.
        flames.head_flame = matches!(
            state,
            FireAntState::Stand | FireAntState::Walk | FireAntState::GetOffButt
        );
        // Sound: EFFECT_BUZZ at the fire ant, pitch kMiddleC-2, while it
        // flies (`UpdateFireAnt`); stop it otherwise.
    }
}

/// What the flames need of a fire ant's model.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (Option<&'static SkeletonRig>, &'static Transform),
    (With<SkeletonAnimator>, Without<FireAntBrain>),
>;

/// Makes the flames the fire ants' moves asked for: the flame on the head
/// of one that is in view, and the fire breath.
///
/// Port of the flame in `UpdateFireAnt` and of `FireAntBreathFire`
/// (original/src/Enemies/Enemy_FireAnt.c). The original breathes before
/// the fire ant's collision and lights the head flame before its transform
/// is updated, so both start where the fire ant was a frame earlier; here
/// they start where it is after this tick's move.
fn emit_fire_ant_flames(
    time: Res<Time>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut fire_ants: Query<(&Transform, &mut FireAntFlames, &EnemyModel)>,
    models: ModelQuery,
    culling: EnemyCulling,
) {
    let dt = time.delta_secs();
    for (transform, mut flames, model) in &mut fire_ants {
        let head = |offset| head_point(&models, model.0, transform, offset);
        if flames.breathe
            && flames.breath_due(dt)
            && let (Some(mouth), Some(center)) = (head(MOUTH_OFFSET), head(HEAD_CENTER_OFFSET))
        {
            breathe_fire(&mut flames, &mut groups, &mut random, mouth, center);
        }
        // The head flame stays out while the fire ant is out of view, so
        // its timer and group don't change then.
        if flames.head_flame && !culling.is_culled(transform.translation, 0.0) {
            light_head_flame(&mut flames, &mut groups, &mut random, dt, || {
                head(HEAD_FLAME_OFFSET)
            });
        }
    }
}

/// Where `offset`, a point in the head joint's space, is in the world.
fn head_point(models: &ModelQuery, model: Entity, root: &Transform, offset: Vec3) -> Option<Vec3> {
    let (rig, model_transform) = models.get(model).ok()?;
    let base = Affine3A::from_rotation_translation(root.rotation, root.translation)
        * model_transform.compute_affine();
    joint_position(rig?, HEAD_JOINT, offset, base)
}

/// Spews a burst of fire from `mouth`, away from the head's `center`, in a
/// cone. When the group is full, the burst goes into a new group.
///
/// Port of `FireAntBreathFire` (original/src/Enemies/Enemy_FireAnt.c),
/// after its regulator.
fn breathe_fire(
    flames: &mut FireAntFlames,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    mouth: Vec3,
    center: Vec3,
) {
    let direction = (mouth - center).normalize_or_zero();
    // A full group gets a new one and the whole burst again
    // (`goto new_pgroup`), once.
    for _ in 0..2 {
        let group = match flames.breath_group.filter(|g| groups.is_valid(*g)) {
            Some(group) => group,
            None => {
                let Some(group) = groups.new_group(BREATH_GROUP) else {
                    flames.breath_group = None;
                    return;
                };
                flames.breath_group = Some(group);
                group
            }
        };
        let mut full = false;
        for _ in 0..BREATH_PARTICLES {
            let speed = BREATH_MIN_SPEED + random.next_f32() * BREATH_EXTRA_SPEED;
            let mut angle = || random.next_f32() * BREATH_SPREAD;
            let (x, y, z) = (angle(), angle(), angle());
            // `Q3Matrix4x4_SetRotate_XYZ`: about x, then y, then z.
            let spread = Quat::from_euler(EulerRot::ZYX, z, y, x);
            let velocity = spread * (direction * speed);
            if groups.add_particle(group, mouth, velocity, BREATH_SCALE, FULL_ALPHA) {
                full = true;
                break;
            }
        }
        if !full {
            return;
        }
        flames.breath_group = None;
    }
}

/// Adds a flame to the head flame now and then, at the point `head`
/// gives. Port of the flame in `UpdateFireAnt`
/// (original/src/Enemies/Enemy_FireAnt.c).
fn light_head_flame(
    flames: &mut FireAntFlames,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    dt: f32,
    head: impl FnOnce() -> Option<Vec3>,
) {
    let group = match flames.head_group.filter(|g| groups.is_valid(*g)) {
        Some(group) => group,
        None => {
            let group = groups.new_group(HEAD_FLAME_GROUP);
            flames.head_group = group;
            let Some(group) = group else {
                return;
            };
            group
        }
    };
    if !flames.head_flame_due(dt) {
        return;
    }
    let Some(at) = head() else {
        return;
    };
    let mut jitter = || {
        Vec3::new(
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
        )
    };
    let point = at + jitter() * HEAD_FLAME_SPREAD;
    let velocity = jitter() * HEAD_FLAME_SPEED + Vec3::Y * HEAD_FLAME_RISE;
    let scale = random.next_f32() + HEAD_FLAME_MIN_SCALE;
    // A full group just misses a flame, as in the original.
    groups.add_particle(group, point, velocity, scale, FULL_ALPHA);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_standing_fire_ant_flies_at_a_close_player_and_breathes_at_one_in_range() {
        assert_eq!(standing_decision(1.0, 400.0), Some(FireAntState::Fly));
        assert_eq!(
            standing_decision(0.01, 800.0),
            Some(FireAntState::BreatheFire)
        );
        assert_eq!(standing_decision(0.1, 800.0), None);
        assert_eq!(standing_decision(0.01, 950.0), None);
    }

    #[test]
    fn a_flying_fire_ant_climbs_to_its_height_and_stops_there() {
        let (mut y, mut vy) = (0.0, 0.0);
        hover(&mut y, &mut vy, FLY_HEIGHT, DT);
        assert!((vy - FLY_CLIMB_ACCEL * DT).abs() < 1e-4);
        assert!(y > 0.0);
        for _ in 0..600 {
            hover(&mut y, &mut vy, FLY_HEIGHT, DT);
        }
        assert_eq!((y, vy), (FLY_HEIGHT, 0.0));
        // And sinks to it from above.
        let (mut y, mut vy) = (FLY_HEIGHT + 100.0, 0.0);
        hover(&mut y, &mut vy, FLY_HEIGHT, DT);
        assert!(vy < 0.0 && y < FLY_HEIGHT + 100.0);
    }

    #[test]
    fn the_breath_comes_in_bursts_and_spreads_from_the_mouth() {
        let mut flames = FireAntFlames::default();
        assert!(!flames.breath_due(0.015));
        assert!(flames.breath_due(0.015));
        assert!(!flames.breath_due(0.015));

        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mouth = Vec3::new(0.0, 100.0, -25.0);
        let center = Vec3::new(0.0, 100.0, 0.0);
        breathe_fire(&mut flames, &mut groups, &mut random, mouth, center);
        let group = flames.breath_group.and_then(|g| groups.get(g));
        let particles = group.map(|g| g.particles().to_vec()).unwrap_or_default();
        assert_eq!(particles.len(), BREATH_PARTICLES);
        for particle in particles {
            assert_eq!(particle.position, mouth);
            let speed = particle.velocity.length();
            assert!((BREATH_MIN_SPEED..=BREATH_MIN_SPEED + BREATH_EXTRA_SPEED).contains(&speed));
            // Within the cone around −Z.
            assert!(particle.velocity.normalize().dot(Vec3::NEG_Z) > 0.95);
        }
        assert!(
            group.is_some_and(|g| g.desc.flags.contains(ParticleFlags::HURT_PLAYER)),
            "the breath hurts"
        );
    }

    #[test]
    fn a_full_breath_group_is_followed_by_a_new_one() {
        let mut flames = FireAntFlames::default();
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let (mouth, center) = (Vec3::NEG_Z, Vec3::ZERO);
        breathe_fire(&mut flames, &mut groups, &mut random, mouth, center);
        let first = flames.breath_group;
        for _ in 0..(crate::effects::MAX_PARTICLES / BREATH_PARTICLES) {
            breathe_fire(&mut flames, &mut groups, &mut random, mouth, center);
        }
        assert_ne!(flames.breath_group, first);
        let count = flames
            .breath_group
            .and_then(|g| groups.get(g))
            .map(|g| g.particles().len());
        assert_eq!(count, Some(BREATH_PARTICLES));
    }

    #[test]
    fn the_head_flame_burns_one_particle_at_a_time() {
        let mut flames = FireAntFlames::default();
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let head = Vec3::new(0.0, 200.0, 0.0);
        light_head_flame(&mut flames, &mut groups, &mut random, 0.005, || Some(head));
        light_head_flame(&mut flames, &mut groups, &mut random, 0.006, || Some(head));
        let group = flames.head_group.and_then(|g| groups.get(g));
        assert_eq!(group.map(|g| g.desc.kind), Some(ParticleKind::Gravitoids));
        let particles = group.map(|g| g.particles().to_vec()).unwrap_or_default();
        assert_eq!(particles.len(), 1);
        let offset = particles[0].position - head;
        assert!(offset.abs().cmple(HEAD_FLAME_SPREAD / 2.0).all());
        assert!(particles[0].velocity.y >= HEAD_FLAME_RISE - HEAD_FLAME_SPEED.y / 2.0);
    }

    /// A fire ant with its model and a player, for the message handlers.
    fn world_with_fire_ant(brain: FireAntBrain) -> (World, Entity, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<BallHitEnemy>>();
        world.init_resource::<Messages<EnemyKilled>>();
        world.init_resource::<ParticleGroups>();
        world.init_resource::<GameRandom>();
        let model = world.spawn(SkeletonAnimator::default()).id();
        let fire_ant = world
            .spawn((
                brain,
                FireAntFlames::default(),
                Transform::default(),
                EnemyModel(model),
                Velocity::default(),
                Health(FIRE_ANT_HEALTH),
            ))
            .id();
        let player = world
            .spawn((Player, Velocity(Vec3::new(100.0, 0.0, 0.0))))
            .id();
        (world, fire_ant, model, player)
    }

    fn ball_hit(player: Entity, enemy: Entity, speed: f32) -> BallHitEnemy {
        BallHitEnemy {
            player,
            enemy,
            ball_velocity: Vec3::new(0.0, 0.0, -speed),
            ball_speed: speed,
        }
    }

    #[test]
    fn only_a_fast_ball_knocks_a_fire_ant_down_once() {
        let (mut world, fire_ant, model, player) = world_with_fire_ant(FireAntBrain::default());
        world.write_message(ball_hit(player, fire_ant, 1000.0));
        world
            .run_system_once(ball_hit_fire_ants)
            .expect("the system runs");
        assert_eq!(
            world.get::<FireAntBrain>(fire_ant).map(|b| b.state),
            Some(FireAntState::Stand)
        );

        world.write_message(ball_hit(player, fire_ant, 1500.0));
        world.write_message(ball_hit(player, fire_ant, 1500.0));
        world
            .run_system_once(ball_hit_fire_ants)
            .expect("the system runs");
        let brain = world
            .get::<FireAntBrain>(fire_ant)
            .copied()
            .unwrap_or_default();
        assert_eq!(brain.state, FireAntState::FallOnButt);
        assert_eq!(brain.butt_timer, BUTT_TIME);
        assert!(!brain.dying);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(FireAntState::FallOnButt.anim())
        );
        assert_eq!(
            world.get::<Velocity>(fire_ant).map(|v| v.0),
            Some(Vec3::new(0.0, 250.0, -1200.0))
        );
        // Only the first knock took health and slowed the player, but both
        // threw sparks.
        let health = world.get::<Health>(fire_ant).map(|h| h.0);
        assert_eq!(health, Some(FIRE_ANT_HEALTH - BALL_DAMAGE));
        let slowed = world.get::<Velocity>(player).map(|v| v.0);
        assert_eq!(slowed, Some(Vec3::new(100.0 * PLAYER_SLOWDOWN, 0.0, 0.0)));
        let groups = world.resource::<ParticleGroups>().iter().count();
        assert_eq!(groups, 4);
    }

    #[test]
    fn a_knock_that_takes_the_last_health_kills_the_fire_ant() {
        let (mut world, fire_ant, _, player) = world_with_fire_ant(FireAntBrain::default());
        world.entity_mut(fire_ant).insert(Health(0.4));
        world.write_message(ball_hit(player, fire_ant, 1500.0));
        world
            .run_system_once(ball_hit_fire_ants)
            .expect("the system runs");
        let brain = world
            .get::<FireAntBrain>(fire_ant)
            .copied()
            .unwrap_or_default();
        assert_eq!(brain.state, FireAntState::FallOnButt);
        assert!(brain.dying);
        assert_eq!(
            world
                .get::<CollisionLayers>(fire_ant)
                .map(|l| l.memberships),
            Some(CollisionKind::Misc.into())
        );
    }

    #[test]
    fn a_killed_fire_ant_falls_on_its_butt_to_die_once() {
        let (mut world, fire_ant, model, _) = world_with_fire_ant(FireAntBrain {
            state: FireAntState::Fly,
            ..default()
        });
        for _ in 0..2 {
            world.write_message(EnemyKilled {
                enemy: fire_ant,
                knock: Vec3::ZERO,
            });
        }
        world
            .run_system_once(kill_hurt_fire_ants)
            .expect("the system runs");
        let brain = world
            .get::<FireAntBrain>(fire_ant)
            .copied()
            .unwrap_or_default();
        assert_eq!(brain.state, FireAntState::FallOnButt);
        assert!(brain.dying);
        assert_eq!(brain.butt_timer, BUTT_TIME);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(FireAntState::FallOnButt.anim())
        );
    }

    #[test]
    fn fire_ants_are_at_night_and_in_the_ant_hill() {
        use bugdom_formats::rsrc::ResourceFork;
        for name in ["Night", "AntHill"] {
            let path = bugdom_formats::original_data_dir().join(format!("Terrain/{name}.ter.rsrc"));
            let fork = ResourceFork::open(&path).expect("terrain file");
            let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
            let items = terrain
                .items
                .iter()
                .filter(|i| i.kind == kind::FIRE_ANT)
                .count();
            assert!(items > 0, "no fire ants in {name}");
        }
    }
}
