//! The worker bee: it walks at the player and, close enough, turns its
//! butt to shoot its stinger, falling flat on its face with the effort.
//! At the ball it doesn't shoot; it pounds it with its fist.
//!
//! Port of original/src/Enemies/Enemy_WorkerBee.c. Worker bees are both
//! map items (`AddEnemy_WorkerBee`) and spline items
//! (`PrimeEnemy_WorkerBee`), under [`kind::WORKER_BEE`]. The stinger is
//! its own entity ([`stinger`]): held on the butt, then a projectile that
//! hurts through the player's collision (`HurtMe` with
//! [`Damage`](crate::combat::Damage)).
//!
//! The bee answers [`BallHitEnemy`] (which only makes it pound sooner) and
//! [`EnemyKilled`]. `DoBugKick` has no worker bee case, so the kick does
//! nothing to it, and it isn't spiked or boppable.

mod stinger;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKilled, EnemyKind,
    EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, apply_friction,
    death_enemy_collision_mask, default_enemy_collision_mask, detach_enemy_from_spline, move_enemy,
    nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind};
use crate::items::pickups::DetonatorsBlown;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::level::CurrentLevel;
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of};
use crate::physics::PreviousPosition;
use crate::player::{BugState, HurtPlayer, Player, PlayerForm};
use crate::skeleton::{
    AnimationFlags, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position,
};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::AppState;
use crate::terrain::TerrainMap;

use stinger::{Stinger, StingerShot, Stingers};

pub struct WorkerBeePlugin;

impl Plugin for WorkerBeePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<StingerShot>()
            .register_item_kind(kind::WORKER_BEE, add_worker_bee)
            .register_spline_item_kind(kind::WORKER_BEE, prime_worker_bee)
            .add_systems(
                FixedUpdate,
                (
                    // The player's collision reaches the bees before they
                    // move, as in the original's frame; a shot stinger
                    // flies in the frame it is shot.
                    (
                        ball_hit_worker_bees,
                        move_worker_bees,
                        stinger::make_shot_sparks,
                        stinger::move_stingers,
                    )
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_worker_bees.in_set(EnemySystems::Killed),
                    move_worker_bees_on_spline.in_set(SplineSystems::Move),
                    // `AlignStingerOnBee`, once every bee has moved.
                    stinger::align_stingers
                        .after(SplineSystems::Move)
                        .after(EnemySystems::Killed)
                        .run_if(in_state(AppState::InGame)),
                ),
            );
    }
}

/// The most worker bees counted at once (`MAX_WORKERBEES`).
const MAX_WORKER_BEES: usize = 4;
/// `WORKERBEE_SCALE`
const WORKER_BEE_SCALE: f32 = 1.5;
const WORKER_BEE_HEALTH: f32 = 1.0;
/// Touching a bee doesn't hurt (`Damage = 0`).
const WORKER_BEE_DAMAGE: f32 = 0.0;
/// The bee's origin is this far below its feet (`WORKERBEE_FOOT_OFFSET`);
/// being negative, it lifts the bee.
const WORKER_BEE_FOOT_OFFSET: f32 = -152.0;
/// The collision box of a bee from a map item.
const WORKER_BEE_BOX: CollisionBox =
    CollisionBox::new(140.0, WORKER_BEE_FOOT_OFFSET, -100.0, 100.0, 100.0, -100.0);
/// The smaller collision box `PrimeEnemy_WorkerBee` gives a bee on a
/// spline.
const WORKER_BEE_SPLINE_BOX: CollisionBox =
    CollisionBox::new(70.0, WORKER_BEE_FOOT_OFFSET, -70.0, 70.0, 70.0, -70.0);
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const WORKER_BEE_SHADOW_SCALE: f32 = 8.0;

/// Speed along a spline, in baked points per second
/// (`WORKERBEE_SPLINE_SPEED`).
const WORKER_BEE_SPLINE_SPEED: f32 = 100.0;
/// How close the player must come, in units, for a standing bee to walk
/// at it (`WORKERBEE_CHASE_DIST`).
const WORKER_BEE_CHASE_DIST: f32 = 900.0;
/// How close the bug must be, in units, for a bee to shoot
/// (`WORKERBEE_ATTACK_DIST`).
const WORKER_BEE_ATTACK_DIST: f32 = 300.0;
/// How close the player must come, in units, for a bee to leave its
/// spline (`WORKERBEE_DETACH_DIST`).
const WORKER_BEE_DETACH_DIST: f32 = 300.0;
/// How fast a bee turns, in radians per second (`WORKERBEE_TURN_SPEED`).
const WORKER_BEE_TURN_SPEED: f32 = 2.4;
/// Walking speed, in units per second (`WORKERBEE_WALK_SPEED`).
const WORKER_BEE_WALK_SPEED: f32 = 600.0;
/// The walk animation's speed (`WORKERBEE_WALK_SPEED * .004`).
const WORKER_BEE_WALK_ANIM_SPEED: f32 = WORKER_BEE_WALK_SPEED * 0.004;

/// How well aimed at the bug a bee must be to shoot, in radians.
const SHOOT_AIM: f32 = 1.1;
/// How well aimed at the ball a bee must be to pound it, in radians.
const POUND_AIM: f32 = 1.4;
/// How close the ball must be, in units, for a bee to pound it.
const POUND_DIST: f32 = 140.0;
/// How long after a pound before a bee can pound again, in seconds.
const POUND_DELAY: f32 = 1.5;
/// The fist the pound hits with: this far along joint 0's space.
const POUND_JOINT: usize = 0;
const POUND_FIST_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -100.0);
/// Half the fist's hit box across, in units. The original's box has no
/// height: its top and bottom are both this far above the fist.
const POUND_FIST_HALF_WIDTH: f32 = 10.0;
const POUND_FIST_RISE: f32 = 10.0;
/// What the pound does to the player.
const POUND_DAMAGE: f32 = 0.4;
/// Seconds of invincibility a pound gives the player.
const POUND_INVINCIBILITY: f32 = 1.1;

/// How fast shooting knocks a bee back on its face, horizontally and
/// upward, in units per second.
const SHOT_RECOIL_SPEED: f32 = 800.0;
const SHOT_RECOIL_RISE: f32 = 300.0;
/// Friction on a bee lying on its face on the ground, per frame at 60 fps.
const DEATH_FRICTION_PER_FRAME: f32 = 60.0;
/// How far from the player a bee on its face must be, as well as out of
/// view, to be deleted, in units.
const DEAD_DELETE_DIST: f32 = 900.0;
/// How fast the ball must go for its hit to make a sound, in units per
/// second.
const BALL_SOUND_SPEED: f32 = 500.0;

/// `LEVEL_NUM_HIVE`, where some bees wait for a detonator.
const HIVE_LEVEL: usize = 5;
/// `LEVEL_NUM_QUEENBEE`, where killed bees always come back.
const QUEEN_BEE_LEVEL: usize = 6;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const WALK_MORPH_RATE: f32 = 8.0;
const SHOOT_MORPH_RATE: f32 = 2.0;
const FALL_ON_FACE_MORPH_RATE: f32 = 3.0;
const POUND_MORPH_RATE: f32 = 5.0;
const STAND_FROM_POUND_MORPH_RATE: f32 = 6.0;

/// The animation flag the butt animation sets when the stinger should go
/// (`ShootButtFlag`), and the pound sets while the fist can hit
/// (`PoundFlag`); both are `Flag[0]`.
const ATTACK_FLAG: usize = 0;

/// What a bee is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state here plays the animation of the
/// same name (`WORKERBEE_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkerBeeState {
    #[default]
    Stand,
    Walk,
    ShootButt,
    /// Fallen on its face after shooting, for good
    /// (`MoveWorkerBee_Death`).
    FallOnFace,
    Pound,
}

impl WorkerBeeState {
    /// The state's animation (`WORKERBEE_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Stand => 0,
            Self::Walk => 1,
            Self::ShootButt => 2,
            Self::FallOnFace => 3,
            Self::Pound => 4,
        }
    }
}

/// A worker bee's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct WorkerBeeBrain {
    pub state: WorkerBeeState,
    /// Seconds until it may pound the ball again (`TimeUntilPound`).
    pub time_until_pound: f32,
    /// The stinger on its butt (`ChainNode`), until shot.
    pub stinger: Option<Entity>,
    /// It has been killed: it is a plain obstacle now. `KillWorkerBee`
    /// changes nothing else, so the bee carries on as before.
    pub killed: bool,
}

impl WorkerBeeBrain {
    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(&mut self, animator: &mut SkeletonAnimator, state: WorkerBeeState, rate: f32) {
        self.state = state;
        animator.morph_to(state.anim(), rate);
    }
}

/// The bee's skeleton as `AddEnemy_WorkerBee` and `PrimeEnemy_WorkerBee`
/// set it up.
fn worker_bee_skeleton(position: Vec2, shape: CollisionBox) -> EnemySkeleton {
    EnemySkeleton::new(
        EnemyKind::WorkerBee,
        SkeletonType::WorkerBee,
        position,
        WORKER_BEE_SCALE,
    )
    .foot_offset(WORKER_BEE_FOOT_OFFSET)
    .collision_box(shape)
    .kickable()
    .health(WORKER_BEE_HEALTH)
    .damage(WORKER_BEE_DAMAGE)
    .shadow(WORKER_BEE_SHADOW_SCALE)
}

/// Whether a bee from a map item with these parameters waits for its
/// detonator: on the Hive, bit 0 of `params[3]` keys it to the detonator
/// `params[0]`.
fn waits_for_detonator(level: usize, params: [u8; 4], blown: &DetonatorsBlown) -> bool {
    level == HIVE_LEVEL && params[3] & 1 != 0 && !blown.is_blown(params[0])
}

/// Port of `AddEnemy_WorkerBee` (original/src/Enemies/Enemy_WorkerBee.c).
fn add_worker_bee(
    In(spawn): In<ItemSpawn>,
    mut enemies: EnemySpawner,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
) -> bool {
    if !enemies.can_spawn(EnemyKind::WorkerBee, MAX_WORKER_BEES) {
        return false;
    }
    if waits_for_detonator(**level, spawn.params, &blown) {
        return false;
    }
    let Some(bee) =
        enemies.spawn(worker_bee_skeleton(spawn.position, WORKER_BEE_BOX).from_item(spawn.index))
    else {
        return false;
    };
    let commands = enemies.commands();
    commands.entity(bee).insert(WorkerBeeBrain::default());
    stinger::give_stinger(commands, bee);
    true
}

/// Port of `PrimeEnemy_WorkerBee` (original/src/Enemies/Enemy_WorkerBee.c):
/// a bee walking its spline. Like every prime routine it doesn't check
/// the enemy counts.
fn prime_worker_bee(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, WORKER_BEE_SPLINE_SPEED);
    let Some(bee) = enemies.spawn(
        worker_bee_skeleton(spawn.position, WORKER_BEE_SPLINE_BOX)
            .on_spline(on_spline)
            .anim(WorkerBeeState::Walk.anim()),
    ) else {
        return false;
    };
    let commands = enemies.commands();
    commands.entity(bee).insert(WorkerBeeBrain {
        state: WorkerBeeState::Walk,
        ..default()
    });
    stinger::give_stinger(commands, bee);
    true
}

/// Kills a bee: it leaves its spline, never comes back (but on the Queen
/// Bee's level) and becomes a plain obstacle. It never deletes the bee,
/// and it carries on moving as it was. Repeats are ignored.
///
/// Port of `KillWorkerBee` (original/src/Enemies/Enemy_WorkerBee.c).
fn kill_worker_bee(
    commands: &mut Commands,
    bee: Entity,
    brain: &mut WorkerBeeBrain,
    level: CurrentLevel,
) {
    if brain.killed {
        return;
    }
    brain.killed = true;
    let mut entity = commands.entity(bee);
    detach_enemy_from_spline(&mut entity);
    if *level != QUEEN_BEE_LEVEL {
        forget_terrain_item(&mut entity);
    }
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
}

/// A bee whose health ran out dies.
///
/// Port of the `ENEMY_KIND_WORKERBEE` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_worker_bees(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    level: Res<CurrentLevel>,
    mut bees: Query<&mut WorkerBeeBrain>,
) {
    for kill in killed.read() {
        if let Ok(mut brain) = bees.get_mut(kill.enemy) {
            kill_worker_bee(&mut commands, kill.enemy, &mut brain, *level);
        }
    }
}

/// The ball can't knock a worker bee down; it only makes it angry enough
/// to pound at once.
///
/// Port of `BallHitWorkerBee` (original/src/Enemies/Enemy_WorkerBee.c).
fn ball_hit_worker_bees(
    mut hits: MessageReader<BallHitEnemy>,
    mut bees: Query<&mut WorkerBeeBrain>,
) {
    for hit in hits.read() {
        let Ok(mut brain) = bees.get_mut(hit.enemy) else {
            continue;
        };
        brain.time_until_pound = 0.0;
        if hit.ball_speed > BALL_SOUND_SPEED {
            // Sound: EFFECT_POUND at the bee, pitch kMiddleC+2, volume 2.0.
        }
    }
}

/// What a walking bee sees of the player it goes for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    form: PlayerForm,
    /// The bug's state, if it has one.
    bug_state: Option<BugState>,
}

/// Whether a walking bee, `aim` radians off the player and `distance`
/// units from it, starts an attack: the stinger at a bug that is standing,
/// walking, kicking or landing, the fist at the ball once it may pound.
///
/// Port of the attack checks in `MoveWorkerBee_Walking`.
fn walking_attack(
    target: Target,
    aim: f32,
    distance: f32,
    time_until_pound: f32,
) -> Option<WorkerBeeState> {
    match target.form {
        PlayerForm::Bug => {
            let shootable = matches!(
                target.bug_state,
                Some(BugState::Stand | BugState::Walk | BugState::Kick | BugState::Land)
            );
            (shootable && aim < SHOOT_AIM && distance < WORKER_BEE_ATTACK_DIST)
                .then_some(WorkerBeeState::ShootButt)
        }
        PlayerForm::Ball => (time_until_pound <= 0.0 && aim < POUND_AIM && distance < POUND_DIST)
            .then_some(WorkerBeeState::Pound),
    }
}

/// The velocity shooting knocks a bee facing `yaw` back with.
fn shot_recoil(yaw: f32) -> Vec3 {
    let back = -yaw_forward(yaw) * SHOT_RECOIL_SPEED;
    Vec3::new(back.x, SHOT_RECOIL_RISE, back.y)
}

/// The fist's hit box around its point (`DoSimpleBoxCollisionAgainstPlayer`
/// in `MoveWorkerBee_Pound`), flat as in the original.
fn fist_box(fist: Vec3) -> CollisionBox {
    let y = fist.y + POUND_FIST_RISE;
    CollisionBox::new(
        y,
        y,
        fist.x - POUND_FIST_HALF_WIDTH,
        fist.x + POUND_FIST_HALF_WIDTH,
        fist.z + POUND_FIST_HALF_WIDTH,
        fist.z - POUND_FIST_HALF_WIDTH,
    )
}

/// Turns toward `target` at [`WORKER_BEE_TURN_SPEED`] for `dt` seconds
/// and returns the angle still to turn (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) -> f32 {
    let (yaw, aim) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        WORKER_BEE_TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
    aim
}

/// Falls under gravity, slowed by friction if `slow`, and moves
/// (`MoveEnemy`).
fn fall_and_move(body: &mut super::EnemyBodyItem, slow: bool, dt: f32) {
    let mut velocity = **body.velocity;
    if slow {
        apply_friction(
            &mut velocity,
            per_frame_friction(DEATH_FRICTION_PER_FRAME),
            dt,
        );
    }
    velocity.y -= ENEMY_GRAVITY * dt;
    **body.velocity = velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// The parts of a bee's model that its move reads.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SkeletonAnimator,
        &'static mut AnimationFlags,
        Option<&'static SkeletonRig>,
        &'static Transform,
    ),
    (Without<WorkerBeeBrain>, Without<Stinger>),
>;

/// The players, as the bees see them.
type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static PlayerForm,
        Option<&'static BugState>,
        &'static CollisionBoxes,
    ),
    (With<Player>, Without<WorkerBeeBrain>),
>;

/// Moves the bees that aren't on a spline, by their state.
///
/// Port of `MoveWorkerBee`, `MoveWorkerBee_Standing`,
/// `MoveWorkerBee_Walking`, `MoveWorkerBee_ShootButt`,
/// `MoveWorkerBee_Death`, `MoveWorkerBee_Pound` and `UpdateWorkerBee`
/// (original/src/Enemies/Enemy_WorkerBee.c). Leaving the item window
/// (`TrackTerrainItem`) is `DespawnOutOfRange`; placing the stinger
/// (`AlignStingerOnBee`) is [`stinger::align_stingers`]. Each bee goes for
/// the nearest player, and looks at that player's form.
#[allow(clippy::too_many_arguments)]
fn move_worker_bees(
    mut commands: Commands,
    mut collision: EnemyCollision,
    level: Res<CurrentLevel>,
    mut bees: Query<
        (EnemyBody, &mut WorkerBeeBrain, &EnemyModel),
        (Without<OnSpline>, Without<Player>),
    >,
    mut models: ModelQuery,
    mut stingers: Stingers,
    players: PlayerQuery,
    culling: EnemyCulling,
    mut hurts: MessageWriter<HurtPlayer>,
    mut shots: MessageWriter<StingerShot>,
) {
    let dt = collision.dt();
    for (mut body, mut brain, model) in &mut bees {
        let bee = body.entity;
        let Ok((mut animator, mut flags, rig, model_transform)) = models.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let player_at = nearest_player(coord, players.iter().map(|(_, t, ..)| t.translation));
        let target = player_at.and_then(|at| {
            players.iter().find(|(_, t, ..)| t.translation == at).map(
                |(_, _, form, bug_state, _)| Target {
                    form: *form,
                    bug_state: bug_state.copied(),
                },
            )
        });
        let player = player_at.unwrap_or(coord);
        let mut mask = default_enemy_collision_mask();
        // Whether a liquid kills the bee (all but the butt, the fall and
        // the pound check).
        let mut drowns = false;
        let mut aim = 0.0;

        // The move, up to the collision.
        match brain.state {
            WorkerBeeState::Stand => {
                drowns = true;
                turn_toward_target(&mut body.transform, player.xz(), dt);
                if quick_distance(player.xz(), coord.xz()) < WORKER_BEE_CHASE_DIST {
                    brain.morph_state(&mut animator, WorkerBeeState::Walk, WALK_MORPH_RATE);
                }
                fall_and_move(&mut body, false, dt);
            }
            WorkerBeeState::Walk => {
                drowns = true;
                aim = turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * WORKER_BEE_WALK_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                fall_and_move(&mut body, false, dt);
                animator.speed = WORKER_BEE_WALK_ANIM_SPEED;
            }
            WorkerBeeState::ShootButt => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                fall_and_move(&mut body, false, dt);
            }
            WorkerBeeState::FallOnFace => {
                let at = body.transform.translation;
                if culling.is_culled(at, **body.radius)
                    && quick_distance(at.xz(), player.xz()) > DEAD_DELETE_DIST
                {
                    commands.entity(bee).despawn();
                    continue;
                }
                let on_ground = body.ground.on_ground;
                fall_and_move(&mut body, on_ground, dt);
                mask = death_enemy_collision_mask();
            }
            WorkerBeeState::Pound => fall_and_move(&mut body, false, dt),
        }

        // Port of `DoEnemyCollisionDetect`. `KillWorkerBee` never deletes,
        // so its effects can wait until the collision ends.
        let state = brain.state;
        let mut killed = false;
        let contact = collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        if killed {
            kill_worker_bee(&mut commands, bee, &mut brain, *level);
        }
        let underwater = drowns && contact.underwater.is_some();
        if underwater {
            kill_worker_bee(&mut commands, bee, &mut brain, *level);
        }

        // What follows the collision, by the state the move started in.
        let at = body.transform.translation;
        match state {
            WorkerBeeState::Walk if !underwater => {
                let distance = quick_distance(player.xz(), at.xz());
                let attack = target.and_then(|target| {
                    walking_attack(target, aim, distance, brain.time_until_pound)
                });
                match attack {
                    Some(WorkerBeeState::ShootButt) => {
                        brain.morph_state(
                            &mut animator,
                            WorkerBeeState::ShootButt,
                            SHOOT_MORPH_RATE,
                        );
                        flags.0[ATTACK_FLAG] = false;
                        body.velocity.x = 0.0;
                        body.velocity.z = 0.0;
                    }
                    Some(WorkerBeeState::Pound) => {
                        brain.morph_state(&mut animator, WorkerBeeState::Pound, POUND_MORPH_RATE);
                        body.velocity.x = 0.0;
                        body.velocity.z = 0.0;
                        flags.0[ATTACK_FLAG] = false;
                    }
                    _ => {}
                }
            }
            WorkerBeeState::ShootButt if flags.0[ATTACK_FLAG] => {
                let yaw = yaw_of(body.transform.rotation);
                stinger::shoot_stinger(&mut commands, &mut brain, &mut stingers, yaw, &mut shots);
                **body.velocity = shot_recoil(yaw);
                brain.morph_state(
                    &mut animator,
                    WorkerBeeState::FallOnFace,
                    FALL_ON_FACE_MORPH_RATE,
                );
            }
            WorkerBeeState::Pound => {
                if flags.0[ATTACK_FLAG]
                    && let Some(rig) = rig
                {
                    // The joints are where the last frame left them, and
                    // so is the bee (`FindCoordOnJoint` reads the last
                    // `UpdateObject`'s matrix).
                    let base = Affine3A::from_rotation_translation(
                        body.transform.rotation,
                        **body.previous,
                    ) * model_transform.compute_affine();
                    if let Some(fist) = joint_position(rig, POUND_JOINT, POUND_FIST_OFFSET, base) {
                        let fist = fist_box(fist);
                        for (player, transform, _, _, boxes) in &players {
                            if boxes
                                .0
                                .iter()
                                .any(|b| b.at(transform.translation).overlaps(&fist))
                            {
                                hurts.write(HurtPlayer {
                                    invincible_for: POUND_INVINCIBILITY,
                                    ..HurtPlayer::new(player, Some(bee), POUND_DAMAGE)
                                });
                            }
                        }
                    }
                }
                if animator.has_stopped {
                    brain.morph_state(
                        &mut animator,
                        WorkerBeeState::Stand,
                        STAND_FROM_POUND_MORPH_RATE,
                    );
                    brain.time_until_pound = POUND_DELAY;
                }
            }
            _ => {}
        }

        // `UpdateWorkerBee`.
        brain.time_until_pound -= dt;
    }
}

/// Moves the bees along their splines, and lets one off to walk at the
/// player once the player is close.
///
/// Port of `MoveWorkerBeeOnSpline` (original/src/Enemies/Enemy_WorkerBee.c).
/// The collision box and shadow follow the transform by themselves, and
/// the stinger is placed by [`stinger::align_stingers`].
fn move_worker_bees_on_spline(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut bees: Query<(
        Entity,
        &mut Transform,
        &mut OnSpline,
        &PreviousPosition,
        &mut WorkerBeeBrain,
        &EnemyModel,
    )>,
    mut animators: Query<&mut SkeletonAnimator, Without<WorkerBeeBrain>>,
    players: Query<&Transform, (With<Player>, Without<WorkerBeeBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (bee, mut transform, mut on_spline, previous, mut brain, model) in &mut bees {
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;
        if !on_spline.visible {
            continue;
        }
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), previous.xz(), position);
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y = map.floor_height(position.x, position.y) - WORKER_BEE_FOOT_OFFSET;

        let at = transform.translation;
        let near = nearest_player(at, players.iter().map(|t| t.translation))
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < WORKER_BEE_DETACH_DIST);
        if near {
            if let Ok(mut animator) = animators.get_mut(model.0) {
                brain.morph_state(&mut animator, WorkerBeeState::Walk, WALK_MORPH_RATE);
            } else {
                brain.state = WorkerBeeState::Walk;
            }
            detach_enemy_from_spline(&mut commands.entity(bee));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const BUG_STANDING: Target = Target {
        form: PlayerForm::Bug,
        bug_state: Some(BugState::Stand),
    };
    const BALL: Target = Target {
        form: PlayerForm::Ball,
        bug_state: Some(BugState::RollUp),
    };

    #[test]
    fn a_walking_bee_shoots_a_close_bug_it_faces() {
        assert_eq!(
            walking_attack(BUG_STANDING, 0.5, 200.0, 0.0),
            Some(WorkerBeeState::ShootButt)
        );
        assert_eq!(walking_attack(BUG_STANDING, 1.2, 200.0, 0.0), None);
        assert_eq!(walking_attack(BUG_STANDING, 0.5, 300.0, 0.0), None);
        for state in [BugState::Walk, BugState::Kick, BugState::Land] {
            let target = Target {
                bug_state: Some(state),
                ..BUG_STANDING
            };
            assert!(walking_attack(target, 0.5, 200.0, 0.0).is_some());
        }
        for state in [BugState::Jump, BugState::Fall, BugState::KnockedOnButt] {
            let target = Target {
                bug_state: Some(state),
                ..BUG_STANDING
            };
            assert_eq!(walking_attack(target, 0.5, 200.0, 0.0), None);
        }
    }

    #[test]
    fn a_walking_bee_pounds_a_close_ball_once_it_may() {
        assert_eq!(
            walking_attack(BALL, 1.3, 100.0, 0.0),
            Some(WorkerBeeState::Pound)
        );
        assert_eq!(walking_attack(BALL, 1.3, 100.0, 0.5), None);
        assert_eq!(walking_attack(BALL, 1.5, 100.0, 0.0), None);
        assert_eq!(walking_attack(BALL, 1.3, 150.0, 0.0), None);
    }

    #[test]
    fn shooting_knocks_the_bee_back_and_up() {
        // Yaw 0 faces -Z, so it is knocked toward +Z.
        let recoil = shot_recoil(0.0);
        assert!((recoil - Vec3::new(0.0, SHOT_RECOIL_RISE, SHOT_RECOIL_SPEED)).length() < 1e-3);
    }

    #[test]
    fn keyed_hive_bees_wait_for_their_detonator() {
        let mut blown = DetonatorsBlown::default();
        let keyed = [2, 0, 0, 1];
        assert!(waits_for_detonator(HIVE_LEVEL, keyed, &blown));
        assert!(!waits_for_detonator(HIVE_LEVEL, [2, 0, 0, 0], &blown));
        assert!(!waits_for_detonator(QUEEN_BEE_LEVEL, keyed, &blown));
        blown.0.insert(2);
        assert!(!waits_for_detonator(HIVE_LEVEL, keyed, &blown));
    }

    #[test]
    fn the_fist_box_is_flat_above_the_fist() {
        let fist = fist_box(Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(fist.top, 30.0);
        assert_eq!(fist.bottom, 30.0);
        assert_eq!((fist.left, fist.right), (0.0, 20.0));
        assert_eq!((fist.back, fist.front), (20.0, 40.0));
    }

    #[test]
    fn killing_a_bee_makes_it_an_obstacle_once() {
        let mut world = World::new();
        let bee = world.spawn_empty().id();
        let mut brain = WorkerBeeBrain::default();
        let result = world.run_system_once(move |mut commands: Commands| {
            kill_worker_bee(&mut commands, bee, &mut brain, CurrentLevel(HIVE_LEVEL));
            let first = brain.killed;
            kill_worker_bee(&mut commands, bee, &mut brain, CurrentLevel(HIVE_LEVEL));
            first && brain.killed
        });
        assert_eq!(result.ok(), Some(true));
        assert!(world.get::<CollisionLayers>(bee).is_some());
    }
}
