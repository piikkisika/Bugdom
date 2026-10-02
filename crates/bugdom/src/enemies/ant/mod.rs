//! The ant: a spear carrier that throws its spear at the player and walks
//! over to pick it up again, or a rock thrower that picks up rocks and
//! lobs them. Knocked down, it falls on its butt; killed, it falls and
//! dies, and in the Ant Hill its ghost rises from the body to fight on.
//!
//! Port of original/src/Enemies/Enemy_Ant.c. Ants are both map items
//! (`AddEnemy_Ant`) and spline items (`PrimeEnemy_Ant`), under
//! [`kind::ANT`]. The spear or rock an ant holds is its own entity
//! ([`items`]), a child of the ant while held, as the original's
//! `ChainNode` is drawn and hidden with the ant; thrown, it flies on its
//! own and hurts the player through the player's collision (`HurtMe` with
//! [`Damage`](crate::combat::Damage)). The ghost is in [`ghost`].
//!
//! The ant answers [`EnemyKicked`] and [`BallHitEnemy`] (both knock it on
//! its butt) and [`EnemyKilled`]. It isn't spiked or boppable, so the
//! player's other contacts don't concern it.

mod ghost;
mod items;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled,
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, KICK_SPEED, apply_friction,
    death_enemy_collision_mask, default_enemy_collision_mask, detach_enemy_from_spline, move_enemy,
    nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides};
use crate::combat::Health;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of};
use crate::objects::ModelSpawner;
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::skeleton::{AnimationFlags, SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::AppState;
use crate::terrain::TerrainMap;

use ghost::GhostAnt;
use items::{AntItem, AntItems, Carried};

pub struct AntPlugin;

impl Plugin for AntPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::ANT, add_ant)
            .register_spline_item_kind(kind::ANT, prime_ant)
            .add_systems(
                FixedUpdate,
                (
                    kick_ants.in_set(EnemySystems::Kicked),
                    // The player's collision reaches the ants before they
                    // move, as in the original's frame.
                    (ball_hit_ants, move_ants, items::move_thrown_items)
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_ants.in_set(EnemySystems::Killed),
                    move_ants_on_spline.in_set(SplineSystems::Move),
                    // `UpdateAntSpear` and `UpdateAntRock`, once every ant
                    // has moved.
                    items::hold_ant_items
                        .after(SplineSystems::Move)
                        .after(EnemySystems::Killed)
                        .run_if(in_state(AppState::InGame)),
                ),
            )
            .add_systems(Update, ghost::make_ghosts_glow);
    }
}

/// The most ants counted at once (`MAX_ANTS`).
const MAX_ANTS: usize = 5;
/// `ANT_SCALE`
const ANT_SCALE: f32 = 1.4;
/// How fast an ant turns, in radians per second (`ANT_TURN_SPEED`).
const ANT_TURN_SPEED: f32 = 2.4;
/// Walking speed, in units per second (`ANT_WALK_SPEED`).
const ANT_WALK_SPEED: f32 = 400.0;
/// The walk animation's speed (`ANT_WALK_SPEED * .0032`).
const ANT_WALK_ANIM_SPEED: f32 = ANT_WALK_SPEED * 0.0032;
/// How fast the ball must go to knock an ant down, in units per second
/// (`ANT_KNOCKDOWN_SPEED`).
const ANT_KNOCKDOWN_SPEED: f32 = 1400.0;
/// What touching an ant does to the player (`ANT_DAMAGE`).
const ANT_DAMAGE: f32 = 0.1;
const ANT_HEALTH: f32 = 1.0;
/// The ant's origin is this far below its feet (`ANT_FOOT_OFFSET`); being
/// negative, it lifts the ant.
const ANT_FOOT_OFFSET: f32 = -150.0;
/// The top of the collision box (`ANT_HEAD_OFFSET`); half of it while on
/// its butt.
const ANT_HEAD_OFFSET: f32 = 80.0;
/// The top of the collision box of an ant made on a spline, which
/// `PrimeEnemy_Ant` sets to 70 rather than [`ANT_HEAD_OFFSET`].
const ANT_SPLINE_HEAD_OFFSET: f32 = 70.0;
/// Half the collision box's width and depth.
const ANT_HALF_WIDTH: f32 = 70.0;
/// How close the player must come, in units, for an ant to leave its
/// spline (`ANT_LEAVE_SPLINE_DIST`).
const ANT_LEAVE_SPLINE_DIST: f32 = 600.0;
/// How close a walking ant gets to the player, in units, before it stands
/// to attack (`ANT_AGGRESSIVE_STOP_DIST`).
const ANT_AGGRESSIVE_STOP_DIST: f32 = ANT_LEAVE_SPLINE_DIST - 200.0;
/// Speed along a spline, in baked points per second
/// (`IncreaseSplineIndex(theNode, 100)`).
const ANT_SPLINE_SPEED: f32 = 100.0;
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const ANT_SHADOW_SCALE: f32 = 8.0;

/// How close the player must be, in units, for an ant to throw
/// (`SPEAR_ATTACK_DIST`).
const SPEAR_ATTACK_DIST: f32 = 900.0;
/// How well aimed at the player an ant must be to throw, in radians
/// (`SPEAR_THROW_MIN_ANGLE`).
const SPEAR_THROW_MIN_ANGLE: f32 = 0.03;
/// How close to its thrown spear an ant must walk to pick it up, in units
/// (`DIST_TO_RETRIVE_SPEAR`).
const DIST_TO_RETRIEVE_SPEAR: f32 = 200.0;

/// How long an ant sits on its butt, in seconds (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// Friction on an ant on its butt on the ground, per frame at 60 fps.
const BUTT_FRICTION_PER_FRAME: f32 = 140.0;
/// Friction while getting up and while dead, per frame at 60 fps.
const GET_UP_FRICTION_PER_FRAME: f32 = 60.0;
/// How far from the player a dead ant must be, as well as out of view,
/// to be deleted, in units.
const DEAD_DELETE_DIST: f32 = 600.0;
/// How long a dead ant in the Ant Hill lies before its ghost rises, in
/// seconds.
const GHOST_DELAY: f32 = 0.5;

/// How much of the ball's velocity a knocked-down ant takes
/// (`BallHitAnt`).
const BALL_KNOCK_SHARE: f32 = 0.8;
/// How fast the ball sends an ant up, on top of its share of the ball's
/// own rise, in units per second.
const BALL_KNOCK_RISE: f32 = 250.0;
/// What the ball does to an ant's health.
const BALL_DAMAGE: f32 = 0.5;
/// How much of its velocity the player keeps after knocking an ant down
/// (`gDelta *= .2` in `KnockAntOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const STAND_FROM_WALK_MORPH_RATE: f32 = 2.0;
const PICKUP_ROCK_MORPH_RATE: f32 = 5.0;
const STAND_FROM_THROW_ROCK_MORPH_RATE: f32 = 3.0;
const FALL_ON_BUTT_MORPH_RATE: f32 = 9.0;

/// The animation flag the throw animations set when the spear or rock
/// should leave the hand, and the pick-up animations set when it should
/// arrive (`ThrowSpear` and `PickUpNow`, both `Flag[0]`).
const THROW_OR_PICK_UP_FLAG: usize = 0;

/// What an ant is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state here plays the animation of the
/// same name (`ANT_ANIM_*`). `ANT_ANIM_WALKCARRY` has no move routine and
/// is never played.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AntState {
    #[default]
    Stand,
    Walk,
    ThrowSpear,
    PickUp,
    FallOnButt,
    GetOffButt,
    Die,
    PickUpRock,
    ThrowRock,
}

impl AntState {
    /// The state's animation (`ANT_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Stand => 0,
            Self::Walk => 1,
            Self::ThrowSpear => 2,
            Self::PickUp => 3,
            Self::FallOnButt => 4,
            Self::GetOffButt => 5,
            Self::Die => 6,
            Self::PickUpRock => 8,
            Self::ThrowRock => 9,
        }
    }
}

/// What a spear ant is up to with its spear (`ANT_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AntMode {
    #[default]
    None,
    /// Watching its thrown spear fly.
    WatchSpear,
    /// Walking to its spear, stuck in the ground.
    GetSpear,
}

/// An ant's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct AntBrain {
    pub state: AntState,
    pub mode: AntMode,
    /// Throws rocks rather than a spear (`RockThrower`).
    pub rock_thrower: bool,
    /// Walks after the player once it has its spear back (`Aggressive`).
    pub aggressive: bool,
    /// Dies once it is off its butt (`Dying`).
    pub dying: bool,
    /// Seconds left on its butt (`ButtTimer`).
    pub butt_timer: f32,
    /// Seconds it has been dead (`DeathTimer`).
    pub death_timer: f32,
    /// Its ghost has risen (`MadeGhost`).
    pub made_ghost: bool,
    /// The spear or rock in its hand (`ChainNode`; `HasSpear` for a spear
    /// ant).
    pub held: Option<Entity>,
    /// The spear it threw, to go and fetch (`ThrownSpear`).
    pub thrown_spear: Option<Entity>,
}

impl AntBrain {
    /// Switches to `state` and starts its animation (`SetSkeletonAnim`).
    fn set_state(&mut self, animator: &mut SkeletonAnimator, state: AntState) {
        self.state = state;
        animator.set_anim(state.anim());
    }

    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(&mut self, animator: &mut SkeletonAnimator, state: AntState, rate: f32) {
        self.state = state;
        animator.morph_to(state.anim(), rate);
    }
}

/// The ant's collision box with its top at `top`.
const fn ant_box(top: f32) -> CollisionBox {
    CollisionBox::new(
        top,
        ANT_FOOT_OFFSET,
        -ANT_HALF_WIDTH,
        ANT_HALF_WIDTH,
        ANT_HALF_WIDTH,
        -ANT_HALF_WIDTH,
    )
}

/// The ant's skeleton as `MakeAntObject` and `PrimeEnemy_Ant` set it up,
/// without the shadow.
fn ant_skeleton(position: Vec2, top: f32) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::Ant, SkeletonType::Ant, position, ANT_SCALE)
        .foot_offset(ANT_FOOT_OFFSET)
        .collision_box(ant_box(top))
        .kickable()
        .solid(SolidSides::NOT_TOP)
        .health(ANT_HEALTH)
        .damage(ANT_DAMAGE)
}

/// Port of `AddEnemy_Ant` and `MakeAntObject`
/// (original/src/Enemies/Enemy_Ant.c). `params[0]` is 1 for a rock
/// thrower, and bit 0 of `params[3]` makes it aggressive.
fn add_ant(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !enemies.can_spawn(EnemyKind::Ant, MAX_ANTS) {
        return false;
    }
    let rock_thrower = spawn.params[0] == 1;
    let Some(ant) = enemies.spawn(
        ant_skeleton(spawn.position, ANT_HEAD_OFFSET)
            .from_item(spawn.index)
            .shadow(ANT_SHADOW_SCALE),
    ) else {
        return false;
    };
    let brain = AntBrain {
        rock_thrower,
        aggressive: spawn.params[3] & 1 != 0,
        ..default()
    };
    let commands = enemies.commands();
    commands.entity(ant).insert(brain);
    if !rock_thrower {
        items::give_item(commands, ant, Carried::Spear);
    }
    true
}

/// Port of `PrimeEnemy_Ant` (original/src/Enemies/Enemy_Ant.c): a spear
/// ant walking its spline, aggressive. Like every prime routine it
/// doesn't check the enemy counts, and it ignores the item's parameters.
fn prime_ant(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, ANT_SPLINE_SPEED);
    let Some(ant) = enemies.spawn(
        ant_skeleton(spawn.position, ANT_SPLINE_HEAD_OFFSET)
            .on_spline(on_spline)
            .anim(AntState::Walk.anim())
            .shadow(ANT_SHADOW_SCALE),
    ) else {
        return false;
    };
    let brain = AntBrain {
        state: AntState::Walk,
        aggressive: true,
        ..default()
    };
    let commands = enemies.commands();
    commands.entity(ant).insert(brain);
    items::give_item(commands, ant, Carried::Spear);
    true
}

/// Sets the top of an ant's collision box (`TopOff`). The collider is
/// rebuilt with it.
fn set_box_top(commands: &mut Commands, ant: Entity, boxes: &CollisionBoxes, top: f32) {
    let mut boxes = boxes.clone();
    if let Some(shape) = boxes.0.first_mut() {
        shape.top = top;
    }
    commands.entity(ant).insert((boxes.collider(), boxes));
}

/// The parts of an ant that its kill and knock change.
struct AntParts<'a> {
    entity: Entity,
    brain: &'a mut AntBrain,
    animator: Option<Mut<'a, SkeletonAnimator>>,
}

/// Kills an ant: it leaves its spline, never comes back, becomes a plain
/// obstacle, and dies once it is off its butt. It never deletes the ant.
/// Repeats are ignored.
///
/// Port of `KillAnt` (original/src/Enemies/Enemy_Ant.c).
fn kill_ant(ant: AntParts, commands: &mut Commands) {
    if ant.brain.dying {
        return;
    }
    let mut entity = commands.entity(ant.entity);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    ant.brain.dying = true;
    if ant.brain.state != AntState::FallOnButt {
        if let Some(mut animator) = ant.animator {
            ant.brain.set_state(&mut animator, AntState::FallOnButt);
        } else {
            ant.brain.state = AntState::FallOnButt;
        }
        ant.brain.butt_timer = BUTT_TIME;
    }
}

/// What [`knock_ant_on_butt`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knock {
    /// It was on its butt already, and nothing happened.
    Ignored,
    /// It went down; the player slows down.
    KnockedDown,
}

/// Knocks an ant on its butt with `velocity`, dropping any rock it holds,
/// and hurts it by `damage`, killing it if that was the last of its
/// health.
///
/// Port of `KnockAntOnButt` (original/src/Enemies/Enemy_Ant.c), but for
/// slowing the player down, which the callers do with the player they
/// know.
#[allow(clippy::too_many_arguments)]
fn knock_ant_on_butt(
    mut ant: AntParts,
    commands: &mut Commands,
    ant_velocity: &mut Velocity,
    health: &mut Health,
    boxes: &CollisionBoxes,
    velocity: Vec3,
    damage: f32,
) -> Knock {
    if ant.brain.state == AntState::FallOnButt {
        return Knock::Ignored;
    }
    detach_enemy_from_spline(&mut commands.entity(ant.entity));
    if ant.brain.rock_thrower
        && let Some(rock) = ant.brain.held.take()
    {
        commands.entity(rock).despawn();
    }
    **ant_velocity = velocity;
    match ant.animator.as_mut() {
        Some(animator) => {
            ant.brain
                .morph_state(animator, AntState::FallOnButt, FALL_ON_BUTT_MORPH_RATE);
        }
        None => ant.brain.state = AntState::FallOnButt,
    }
    ant.brain.butt_timer = BUTT_TIME;
    set_box_top(commands, ant.entity, boxes, ANT_HEAD_OFFSET / 2.0);
    if health.lose(damage) {
        kill_ant(ant, commands);
    }
    Knock::KnockedDown
}

/// What the kick and ball handlers need of an ant.
type KnockQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut AntBrain,
        &'static mut Velocity,
        &'static mut Health,
        &'static EnemyModel,
        &'static CollisionBoxes,
    ),
    Without<Player>,
>;

/// The players' velocities, which knocking an ant down slows.
type PlayerVelocities<'w, 's> =
    Query<'w, 's, &'static mut Velocity, (With<Player>, Without<AntBrain>)>;

/// The kick knocks an ant on its butt.
///
/// Port of the `SKELETON_TYPE_ANT` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_ants(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut ants: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<AntBrain>>,
    mut players: PlayerVelocities,
) {
    for kick in kicks.read() {
        let Ok((mut brain, mut velocity, mut health, model, boxes)) = ants.get_mut(kick.enemy)
        else {
            continue;
        };
        let ant = AntParts {
            entity: kick.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok(),
        };
        let knock = knock_ant_on_butt(
            ant,
            &mut commands,
            &mut velocity,
            &mut health,
            boxes,
            kick.knock(KICK_SPEED, KICK_SPEED),
            kick.damage,
        );
        if knock == Knock::KnockedDown
            && let Ok(mut player) = players.get_mut(kick.player)
        {
            **player *= PLAYER_SLOWDOWN;
        }
    }
}

/// A fast enough ball knocks an ant on its butt.
///
/// Port of `BallHitAnt` (original/src/Enemies/Enemy_Ant.c).
fn ball_hit_ants(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut ants: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<AntBrain>>,
    mut players: PlayerVelocities,
) {
    for hit in hits.read() {
        if hit.ball_speed <= ANT_KNOCKDOWN_SPEED {
            continue;
        }
        let Ok((mut brain, mut velocity, mut health, model, boxes)) = ants.get_mut(hit.enemy)
        else {
            continue;
        };
        let ant = AntParts {
            entity: hit.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok(),
        };
        let knock = knock_ant_on_butt(
            ant,
            &mut commands,
            &mut velocity,
            &mut health,
            boxes,
            ball_knock(hit.ball_velocity),
            BALL_DAMAGE,
        );
        if knock == Knock::KnockedDown {
            // Sound: EFFECT_POUND at the ant, pitch kMiddleC+2, volume 2.0.
            if let Ok(mut player) = players.get_mut(hit.player) {
                **player *= PLAYER_SLOWDOWN;
            }
        }
    }
}

/// The velocity a ball going at `ball_velocity` knocks an ant over with.
fn ball_knock(ball_velocity: Vec3) -> Vec3 {
    ball_velocity * BALL_KNOCK_SHARE + Vec3::Y * BALL_KNOCK_RISE
}

/// An ant whose health ran out dies.
///
/// Port of the `ENEMY_KIND_ANT` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_ants(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut ants: Query<(&mut AntBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator, Without<AntBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = ants.get_mut(kill.enemy) else {
            continue;
        };
        let ant = AntParts {
            entity: kill.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok(),
        };
        kill_ant(ant, &mut commands);
    }
}

/// What a standing ant decides to do about the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StandingAttack {
    None,
    PickUpRock,
    ThrowSpear,
}

/// Whether a standing ant, `aim` radians off the player and `distance`
/// units from it, starts an attack. Port of the attack check in
/// `MoveAnt_Standing`.
fn standing_attack(brain: &AntBrain, aim: f32, distance: f32) -> StandingAttack {
    let in_reach = aim < SPEAR_THROW_MIN_ANGLE && distance < SPEAR_ATTACK_DIST;
    if brain.rock_thrower {
        if in_reach {
            return StandingAttack::PickUpRock;
        }
    } else if brain.held.is_some() && in_reach {
        return StandingAttack::ThrowSpear;
    }
    StandingAttack::None
}

/// Turns toward `target` at [`ANT_TURN_SPEED`] for `dt` seconds and
/// returns the angle still to turn (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) -> f32 {
    let (yaw, aim) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        ANT_TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
    aim
}

/// Walks forward at [`ANT_WALK_SPEED`] under gravity (the walking part of
/// `MoveAnt_Walking`).
fn walk_forward(body: &mut super::EnemyBodyItem, dt: f32) {
    let forward = yaw_forward(yaw_of(body.transform.rotation)) * ANT_WALK_SPEED;
    let mut velocity = **body.velocity;
    velocity.x = forward.x;
    velocity.z = forward.y;
    velocity.y -= ENEMY_GRAVITY * dt;
    **body.velocity = velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// Falls under gravity, slowed by `friction` (per frame at 60 fps) if
/// `slow`: the move of an ant on its butt, getting up or dead.
fn tumble(body: &mut super::EnemyBodyItem, friction: f32, slow: bool, dt: f32) {
    let mut velocity = **body.velocity;
    if slow {
        apply_friction(&mut velocity, per_frame_friction(friction), dt);
    }
    velocity.y -= ENEMY_GRAVITY * dt;
    **body.velocity = velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// The thrown spear `brain` remembers, if it is still that ant's
/// (`IsThrownSpearValid`).
fn thrown_spear(
    ant: Entity,
    brain: &AntBrain,
    items: &AntItems,
) -> Option<(Entity, Vec3, AntItem)> {
    let spear = brain.thrown_spear?;
    let (transform, item) = items.get(spear).ok()?;
    (item.owner == ant).then_some((spear, transform.translation, *item))
}

/// Moves the ants that aren't on a spline, by their state.
///
/// Port of `MoveAnt`, `MoveAnt_Standing`, `MoveAnt_Walking`,
/// `MoveAnt_Throw`, `MoveAnt_PickUp`, `MoveAnt_FallOnButt`,
/// `MoveAnt_GetOffButt`, `MoveAnt_Death`, `MoveAnt_PickupRock` and
/// `MoveAnt_ThrowRock` (original/src/Enemies/Enemy_Ant.c). Leaving the
/// item window (`TrackTerrainItem`) is `DespawnOutOfRange`; placing what
/// the ant holds (`UpdateAnt`) is [`items::hold_ant_items`]. Each ant goes
/// for the nearest player.
#[allow(clippy::too_many_arguments)]
fn move_ants(
    mut commands: Commands,
    mut collision: EnemyCollision,
    mut models: ModelSpawner,
    level: Res<CurrentLevel>,
    mut ants: Query<(EnemyBody, &mut AntBrain, &EnemyModel), (Without<OnSpline>, Without<Player>)>,
    mut animators: Query<(&mut SkeletonAnimator, &mut AnimationFlags), Without<AntBrain>>,
    mut items: AntItems,
    players: Query<&Transform, (With<Player>, Without<AntBrain>, Without<AntItem>)>,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    let in_ant_hill = level.def().level_type == LevelType::AntHill;
    for (mut body, mut brain, model) in &mut ants {
        let ant = body.entity;
        let Ok((mut animator, mut flags)) = animators.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let player = nearest_player(coord, player_positions.iter().copied()).unwrap_or(coord);
        let mut mask = default_enemy_collision_mask();
        // Whether the ant drowns if the collision finds it in a liquid
        // (only standing and walking ants check).
        let mut drowns = false;

        match brain.state {
            AntState::Stand => {
                drowns = true;
                match brain.mode {
                    AntMode::WatchSpear => match thrown_spear(ant, &brain, &items) {
                        None => {
                            brain.mode = AntMode::None;
                            brain.thrown_spear = None;
                        }
                        Some((.., spear)) if spear.is_in_ground() => {
                            brain.mode = AntMode::GetSpear;
                            brain.set_state(&mut animator, AntState::Walk);
                        }
                        Some(_) => {}
                    },
                    _ => {
                        let aim = turn_toward_target(&mut body.transform, player.xz(), dt);
                        let distance = quick_distance(player.xz(), coord.xz());
                        match standing_attack(&brain, aim, distance) {
                            StandingAttack::PickUpRock => {
                                brain.morph_state(
                                    &mut animator,
                                    AntState::PickUpRock,
                                    PICKUP_ROCK_MORPH_RATE,
                                );
                                flags.0[THROW_OR_PICK_UP_FLAG] = false;
                            }
                            StandingAttack::ThrowSpear => {
                                brain.set_state(&mut animator, AntState::ThrowSpear);
                                flags.0[THROW_OR_PICK_UP_FLAG] = false;
                            }
                            StandingAttack::None => {}
                        }
                    }
                }
            }
            AntState::Walk => {
                drowns = true;
                animator.speed = ANT_WALK_ANIM_SPEED;
                match brain.mode {
                    AntMode::GetSpear => match thrown_spear(ant, &brain, &items) {
                        None => {
                            brain.thrown_spear = None;
                            brain.mode = AntMode::None;
                        }
                        Some((_, spear_at, _)) => {
                            turn_toward_target(&mut body.transform, spear_at.xz(), dt);
                            walk_forward(&mut body, dt);
                            let at = body.transform.translation;
                            if quick_distance(at.xz(), spear_at.xz()) < DIST_TO_RETRIEVE_SPEAR {
                                brain.mode = AntMode::None;
                                flags.0[THROW_OR_PICK_UP_FLAG] = false;
                                brain.set_state(&mut animator, AntState::PickUp);
                            }
                        }
                    },
                    _ => {
                        turn_toward_target(&mut body.transform, player.xz(), dt);
                        walk_forward(&mut body, dt);
                        let at = body.transform.translation;
                        if quick_distance(at.xz(), player.xz()) < ANT_AGGRESSIVE_STOP_DIST {
                            brain.morph_state(
                                &mut animator,
                                AntState::Stand,
                                STAND_FROM_WALK_MORPH_RATE,
                            );
                        }
                    }
                }
            }
            AntState::ThrowSpear => {
                let aim = turn_toward_target(&mut body.transform, player.xz(), dt);
                if flags.0[THROW_OR_PICK_UP_FLAG] {
                    flags.0[THROW_OR_PICK_UP_FLAG] = false;
                    if aim < SPEAR_THROW_MIN_ANGLE {
                        throw_spear(
                            &mut commands,
                            &mut models,
                            ant,
                            &mut brain,
                            &mut items,
                            yaw_of(body.transform.rotation),
                            player,
                        );
                    }
                }
                if animator.has_stopped {
                    brain.set_state(&mut animator, AntState::Stand);
                }
            }
            AntState::PickUp => {
                body.velocity.x = 0.0;
                body.velocity.z = 0.0;
                if flags.0[THROW_OR_PICK_UP_FLAG] {
                    flags.0[THROW_OR_PICK_UP_FLAG] = false;
                    if let Some((spear, ..)) = thrown_spear(ant, &brain, &items) {
                        commands.entity(spear).despawn();
                    }
                    brain.thrown_spear = None;
                    // A new spear, even if the old one had gone.
                    items::give_item(&mut commands, ant, Carried::Spear);
                }
                if animator.has_stopped {
                    if brain.aggressive {
                        brain.set_state(&mut animator, AntState::Walk);
                        brain.mode = AntMode::None;
                    } else {
                        brain.set_state(&mut animator, AntState::Stand);
                    }
                }
            }
            AntState::FallOnButt => {
                let on_ground = body.ground.on_ground;
                tumble(&mut body, BUTT_FRICTION_PER_FRAME, on_ground, dt);
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    if brain.dying {
                        brain.set_state(&mut animator, AntState::Die);
                        brain.death_timer = 0.0;
                        brain.made_ghost = false;
                        // Straight to `UpdateAnt`, without the collision.
                        continue;
                    }
                    brain.set_state(&mut animator, AntState::GetOffButt);
                }
            }
            AntState::GetOffButt => {
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, true, dt);
                if animator.has_stopped {
                    set_box_top(&mut commands, ant, body.boxes, ANT_HEAD_OFFSET);
                    match brain.mode {
                        AntMode::GetSpear => brain.set_state(&mut animator, AntState::Walk),
                        AntMode::WatchSpear => {
                            brain.mode = AntMode::GetSpear;
                            brain.set_state(&mut animator, AntState::Walk);
                        }
                        AntMode::None => brain.set_state(&mut animator, AntState::Stand),
                    }
                }
            }
            AntState::Die => {
                let at = body.transform.translation;
                if culling.is_culled(at, **body.radius)
                    && quick_distance(at.xz(), player.xz()) > DEAD_DELETE_DIST
                {
                    commands.entity(ant).despawn();
                    continue;
                }
                let on_ground = body.ground.on_ground;
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, on_ground, dt);
                mask = death_enemy_collision_mask() | CollisionKind::Liquid;
            }
            AntState::PickUpRock => {
                body.velocity.x = 0.0;
                body.velocity.z = 0.0;
            }
            AntState::ThrowRock => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
            }
        }

        // What follows the collision goes by the state the move started
        // in, as the original's move routines carry on after it.
        let state = brain.state;
        // Port of `DoEnemyCollisionDetect`. `KillAnt` never deletes, so
        // its effects can wait until the collision ends.
        let mut killed = false;
        let contact = collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        let underwater = contact.underwater.is_some();
        if killed || (drowns && underwater) {
            let parts = AntParts {
                entity: ant,
                brain: &mut brain,
                animator: Some(animator.reborrow()),
            };
            kill_ant(parts, &mut commands);
        }

        match state {
            AntState::Die if in_ant_hill && !brain.made_ghost && !underwater => {
                // No ghost from a body in a liquid, which would drown the
                // ghost and rise again for ever.
                brain.death_timer += dt;
                if brain.death_timer > GHOST_DELAY {
                    brain.made_ghost = true;
                    ghost::make_ghost_ant(
                        &mut commands,
                        GhostAnt {
                            at: body.transform.translation,
                            rock_thrower: brain.rock_thrower,
                            aggressive: brain.aggressive,
                            animator: animator.clone(),
                        },
                    );
                }
            }
            // The pick-up and throw of a rock come after the collision.
            AntState::PickUpRock => {
                if flags.0[THROW_OR_PICK_UP_FLAG] {
                    flags.0[THROW_OR_PICK_UP_FLAG] = false;
                    if brain.held.is_none() {
                        items::give_item(&mut commands, ant, Carried::Rock);
                    }
                }
                if animator.has_stopped {
                    brain.set_state(&mut animator, AntState::ThrowRock);
                }
            }
            AntState::ThrowRock => {
                if flags.0[THROW_OR_PICK_UP_FLAG] {
                    flags.0[THROW_OR_PICK_UP_FLAG] = false;
                    throw_rock(
                        &mut commands,
                        &mut models,
                        &mut brain,
                        &mut items,
                        yaw_of(body.transform.rotation),
                        player,
                    );
                }
                if animator.has_stopped {
                    brain.morph_state(
                        &mut animator,
                        AntState::Stand,
                        STAND_FROM_THROW_ROCK_MORPH_RATE,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Throws the ant's spear at the player and watches it go.
///
/// Port of `AntThrowSpear` (original/src/Enemies/Enemy_Ant.c).
fn throw_spear(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    ant: Entity,
    brain: &mut AntBrain,
    items: &mut AntItems,
    ant_yaw: f32,
    player: Vec3,
) {
    let Some(spear) = brain.held.take() else {
        return;
    };
    let Ok((mut transform, mut item)) = items.get_mut(spear) else {
        return;
    };
    brain.thrown_spear = Some(spear);
    item.owner = ant;
    items::throw_item(
        commands,
        models,
        spear,
        &mut transform,
        &mut item,
        ant_yaw,
        player,
    );
    brain.mode = AntMode::WatchSpear;
}

/// Lobs the ant's rock at the player.
///
/// Port of `AntThrowRock` (original/src/Enemies/Enemy_Ant.c).
fn throw_rock(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    brain: &mut AntBrain,
    items: &mut AntItems,
    ant_yaw: f32,
    player: Vec3,
) {
    let Some(rock) = brain.held.take() else {
        return;
    };
    let Ok((mut transform, mut item)) = items.get_mut(rock) else {
        return;
    };
    items::throw_item(
        commands,
        models,
        rock,
        &mut transform,
        &mut item,
        ant_yaw,
        player,
    );
}

/// Moves the ants along their splines, and lets one off to walk at the
/// player once the player is close.
///
/// Port of `MoveAntOnSpline` (original/src/Enemies/Enemy_Ant.c). The
/// collision box and shadow follow the transform by themselves, and what
/// the ant holds is placed by [`items::hold_ant_items`].
fn move_ants_on_spline(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut ants: Query<(
        Entity,
        &mut Transform,
        &mut OnSpline,
        &PreviousPosition,
        &mut AntBrain,
    )>,
    players: Query<&Transform, (With<Player>, Without<AntBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (ant, mut transform, mut on_spline, previous, mut brain) in &mut ants {
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;
        if !on_spline.visible {
            continue;
        }
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), previous.xz(), position);
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y = map.floor_height(position.x, position.y) - ANT_FOOT_OFFSET;

        let at = transform.translation;
        let near = nearest_player(at, players.iter().map(|t| t.translation))
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < ANT_LEAVE_SPLINE_DIST);
        if near {
            // Off the spline it walks (`MoveAnt` with the walk animation
            // it has had since `PrimeEnemy_Ant`).
            detach_enemy_from_spline(&mut commands.entity(ant));
            brain.mode = AntMode::None;
        }
    }
}

#[cfg(test)]
mod tests;
