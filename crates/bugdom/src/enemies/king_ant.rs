//! The king ant, the boss of the last level. Wreathed in fire, he walks at
//! the player and shoots fireballs from his staff. Fire makes him
//! unbeatable: the ball and the kick only make him laugh. Once water from a
//! king water pipe puts his fire out, for a few seconds he can be knocked on
//! his butt and hurt. When he dies, the level, and the game, is won.
//!
//! Port of original/src/Enemies/Enemy_KingAnt.c. He is a map item
//! (`AddEnemy_KingAnt`) under [`kind::KING_ANT`]. The water that puts his
//! fire out is any particle group with [`ParticleFlags::EXTINGUISH`]: the
//! king water pipe's spray (`MoveKingWaterPipe`, original/src/Items/Triggers2.c)
//! is one, so the pipe needs no hook into this plugin. The staff, its
//! flame and its fireballs are in [`staff`].
//!
//! He has a [`BossHealthBar`], which the infobar's boss bar follows
//! (`gAntKingObj->Health / ANTKING_HEALTH` in `ShowBossHealth`).

mod staff;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    BallHitEnemy, BoundingRadius, ENEMY_GRAVITY, EnemyBody, EnemyBodyItem, EnemyCollision,
    EnemyCulling, EnemyKicked, EnemyKilled, EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner,
    EnemySystems, apply_friction, death_enemy_collision_mask, default_enemy_collision_mask,
    move_enemy, nearest_player, per_frame_friction,
};
use crate::assets::skeleton::SkeletonAsset;
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, CollisionSystems};
use crate::combat::{BossHealthBar, Health};
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture, particle_hit,
};
use crate::items::{
    AreaCompleted, DespawnOutOfRange, ItemSpawn, RegisterItemKind, forget_terrain_item, kind,
};
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_forward, yaw_of};
use crate::objects::ObjectMaterial;
use crate::physics::Velocity;
use crate::player::Player;
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::state::AppState;

use staff::{KingStaff, Staff};

pub struct KingAntPlugin;

impl Plugin for KingAntPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::KING_ANT, add_king_ant)
            .add_systems(
                FixedUpdate,
                (
                    kick_king_ants.in_set(EnemySystems::Kicked),
                    // The player's collision reaches the king before he
                    // moves, as in the original's frame; `UpdateKingAnt`
                    // ends his move.
                    (ball_hit_king_ants, move_king_ants, update_king_ants)
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_king_ants.in_set(EnemySystems::Killed),
                    // The fireballs fly before the player moves, so that the
                    // player's collision with them sees how far they came
                    // this tick, as the spider's web bullets do.
                    staff::move_staff_bullets
                        .before(CollisionSystems::Gather)
                        .run_if(in_state(AppState::InGame)),
                ),
            )
            .add_systems(
                Update,
                (staff::make_staffs_glow, light_fiery_parts).run_if(in_state(AppState::InGame)),
            );
    }
}

/// `KINGANT_SCALE`
const KING_ANT_SCALE: f32 = 1.5;
/// "LOTS of health!" (`ANTKING_HEALTH`, original/src/Headers/enemy.h).
const KING_ANT_HEALTH: f32 = 5.0;
/// Touching the king does no harm (`Damage = 0`); his fire and his
/// fireballs do.
const KING_ANT_DAMAGE: f32 = 0.0;
/// The king's origin is this far below his feet (`KINGANT_FOOT_OFFSET`);
/// being negative, it lifts him.
const KING_ANT_FOOT_OFFSET: f32 = -220.0;
/// The top of his collision box, in units above the origin.
const KING_ANT_HEAD_OFFSET: f32 = 100.0;
/// Half his collision box's width and depth, in units.
const KING_ANT_HALF_WIDTH: f32 = 110.0;
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const KING_ANT_SHADOW_SCALE: f32 = 8.0;

/// How close the player must come, in units, for the waiting king to walk
/// at it (`KINGANT_CHASE_DIST`).
const CHASE_DIST: f32 = 1600.0;
/// How close the player must be, in units, for the walking king to stop
/// and charge his staff (`KINGANT_ATTACK_DIST`).
const ATTACK_DIST: f32 = 600.0;
/// How fast the king turns, in radians per second (`KINGANT_TURN_SPEED`).
const TURN_SPEED: f32 = 2.0;
/// Walking speed, in units per second (`KINGANT_WALK_SPEED`).
const WALK_SPEED: f32 = 500.0;
/// How long the staff charges before it fires, in seconds
/// (`gStaffCharge > 2.0`).
const STAFF_CHARGE_TIME: f32 = 2.0;

/// How fast the ball must go to knock the wet king down, in units per
/// second (`KINGANT_KNOCKDOWN_SPEED`).
const KNOCKDOWN_SPEED: f32 = 1400.0;
/// How much of the ball's horizontal velocity a knocked-down king takes
/// (`BallHitKingAnt`).
const BALL_KNOCK_SHARE: f32 = 0.3;
/// What the ball does to the king's health (`KnockKingAntOnButt(..., .6)`).
const BALL_DAMAGE: f32 = 0.6;
/// How fast the kick sends the king back, in units per second
/// (`DoBugKick`'s `300.0f` for him).
const KICK_KNOCK_SPEED: f32 = 300.0;
/// How fast a knock sends the king up, in units per second (`Delta.y =
/// 600`).
const KNOCK_RISE: f32 = 600.0;
/// How much of its velocity the player keeps after knocking the king down
/// (`gDelta *= .2` in `KnockKingAntOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;

/// How long a fire stays out once water hits the king, in seconds
/// (`WetTimer = 5`).
const WET_TIME: f32 = 5.0;
/// How long the king sits on his butt, in seconds (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// How long the king lies dead before the area is completed, in seconds
/// (`DeathTimer = 4.0`).
const DEATH_TIME: f32 = 4.0;
/// Friction while charging, on his butt on the ground and dead, per frame
/// at 60 fps (`ApplyFrictionToDeltas(60.0, ...)`).
const FRICTION_PER_FRAME: f32 = 60.0;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const WALK_MORPH_RATE: f32 = 5.0;
const ATTACK_MORPH_RATE: f32 = 5.0;
const WAIT_AFTER_SHOT_MORPH_RATE: f32 = 3.0;
const WAIT_AFTER_BUTT_MORPH_RATE: f32 = 6.0;
const BUTT_MORPH_RATE: f32 = 8.0;
const DEATH_MORPH_RATE: f32 = 2.0;

/// The head's joint (`KINGANT_HEAD_LIMB`).
const HEAD_JOINT: usize = 1;
/// The top of the head, in the head joint's space, where his hair burns.
const HEAD_FLAME_OFFSET: Vec3 = Vec3::new(0.0, 100.0, -50.0);
/// Seconds between the head flame's particles (`FireTimer > .01`).
const HEAD_FLAME_INTERVAL: f32 = 0.01;
/// How far each flame starts from the top of the head, in units (the whole
/// width).
const HEAD_FLAME_SPREAD: Vec3 = Vec3::new(100.0, 40.0, 100.0);
/// The flames' largest speed in each direction, in units per second (the
/// whole width), and the speed they rise at on average.
const HEAD_FLAME_SPEED: Vec3 = Vec3::new(70.0, 40.0, 70.0);
const HEAD_FLAME_RISE: f32 = 90.0;
/// The smallest flame's scale; they are up to 1 bigger.
const HEAD_FLAME_MIN_SCALE: f32 = 1.7;
/// The fire on his head (`UpdateKingAnt`).
const HEAD_FLAME_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Gravitoids,
    flags: ParticleFlags(ParticleFlags::ROOF.0 | ParticleFlags::HOT.0),
    gravity: 0.0,
    magnetism: 14000.0,
    base_scale: 20.0,
    decay_rate: 0.6,
    fade_rate: 0.0,
    texture: ParticleTexture::Fire,
};

/// The parts of his model that burn, drawn without the level's lights:
/// eyebrows, hair and beard (the decomposed meshes `LoadASkeleton` makes
/// null-shaded, original/src/Skeleton/SkeletonObj.c).
const FIERY_PARTS: [usize; 3] = [3, 8, 9];

/// What the king is doing. The original dispatches on his animation
/// (`myMoveTable[AnimNum]`); each state plays the animation of the same
/// name (`KINGANT_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KingAntState {
    #[default]
    Wait,
    Walk,
    /// Standing, charging his staff (`KING_ANIM_ATTACK`).
    Attack,
    OnButt,
    Death,
}

impl KingAntState {
    /// The state's animation (`KINGANT_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Wait => 0,
            Self::Walk => 1,
            Self::Attack => 2,
            Self::OnButt => 3,
            Self::Death => 4,
        }
    }
}

/// The king's own state, on his root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct KingAntBrain {
    pub state: KingAntState,
    /// Seconds his fire stays out (`WetTimer`). While it is above zero, he
    /// can be hurt and doesn't attack.
    pub wet_timer: f32,
    /// Seconds left on his butt (`ButtTimer`).
    pub butt_timer: f32,
    /// Seconds left before the area is completed, once dead (`DeathTimer`).
    pub death_timer: f32,
    /// Seconds his staff has charged (`gStaffCharge`, a global in the
    /// original, which has only one king).
    pub staff_charge: f32,
    /// Seconds since the last head flame particle (`FireTimer`).
    head_flame_timer: f32,
    /// The head flame's group (`ParticleGroup`).
    head_flame_group: Option<ParticleGroupId>,
}

impl KingAntBrain {
    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(
        &mut self,
        animator: Option<&mut SkeletonAnimator>,
        state: KingAntState,
        rate: f32,
    ) {
        self.state = state;
        if let Some(animator) = animator {
            animator.morph_to(state.anim(), rate);
        }
    }

    pub fn is_wet(&self) -> bool {
        self.wet_timer > 0.0
    }
}

/// A burning mesh of the king's model, already given its unlit material.
#[derive(Component, Debug, Clone, Copy)]
struct FieryPart;

/// Port of `AddEnemy_KingAnt` (original/src/Enemies/Enemy_KingAnt.c). An
/// item with any bit of `params[3]` set isn't the real king: it adds
/// nothing, but counts as added. There is no guard on the enemy counts.
///
/// The king never leaves (`MoveKingAnt` doesn't track its item), so he
/// doesn't despawn out of range.
fn add_king_ant(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if spawn.params[3] != 0 {
        return true;
    }
    let Some(king) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::KingAnt,
            SkeletonType::KingAnt,
            spawn.position,
            KING_ANT_SCALE,
        )
        .from_item(spawn.index)
        .foot_offset(KING_ANT_FOOT_OFFSET)
        .collision_box(CollisionBox::new(
            KING_ANT_HEAD_OFFSET,
            KING_ANT_FOOT_OFFSET,
            -KING_ANT_HALF_WIDTH,
            KING_ANT_HALF_WIDTH,
            KING_ANT_HALF_WIDTH,
            -KING_ANT_HALF_WIDTH,
        ))
        .with_kinds(CollisionKind::Kickable)
        .health(KING_ANT_HEALTH)
        .damage(KING_ANT_DAMAGE)
        .anim(KingAntState::Wait.anim())
        .shadow(KING_ANT_SHADOW_SCALE),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(king)
        .remove::<DespawnOutOfRange>()
        .insert((
            KingAntBrain::default(),
            BossHealthBar {
                full: KING_ANT_HEALTH,
            },
        ));
    staff::give_staff(enemies.commands(), king);
    true
}

/// Kills the king: he never comes back, becomes a plain obstacle, and
/// plays his death, after which the area is completed.
///
/// Port of `KillKingAnt` (original/src/Enemies/Enemy_KingAnt.c). Like the
/// original, it doesn't ignore repeats: a second kill starts the death
/// over.
fn kill_king_ant(
    commands: &mut Commands,
    king: Entity,
    brain: &mut KingAntBrain,
    animator: Option<&mut SkeletonAnimator>,
) {
    // Sound: stop the king's crackle (`EffectChannel`).
    let mut entity = commands.entity(king);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    brain.morph_state(animator, KingAntState::Death, DEATH_MORPH_RATE);
    brain.death_timer = DEATH_TIME;
}

/// Knocks the king on his butt with horizontal `knock` and hurts him by
/// `damage`, killing him if that was the last of his health. Only a wet
/// king can be knocked down, and not twice. Returns whether he was knocked
/// down, so that the caller slows down the player who did it.
///
/// Port of `KnockKingAntOnButt` (original/src/Enemies/Enemy_KingAnt.c), but
/// for slowing the player down, which the caller does with the player it
/// knows.
#[allow(clippy::too_many_arguments)]
fn knock_king_ant_on_butt(
    commands: &mut Commands,
    king: Entity,
    brain: &mut KingAntBrain,
    mut animator: Option<&mut SkeletonAnimator>,
    velocity: &mut Velocity,
    health: &mut Health,
    knock: Vec2,
    damage: f32,
) -> bool {
    if !brain.is_wet() {
        // Sound: EFFECT_KINGLAUGH at the king, pitch kMiddleC, volume 2.0.
        return false;
    }
    if brain.state == KingAntState::OnButt {
        return false;
    }
    brain.morph_state(
        animator.as_deref_mut(),
        KingAntState::OnButt,
        BUTT_MORPH_RATE,
    );
    brain.butt_timer = BUTT_TIME;
    **velocity = Vec3::new(knock.x, KNOCK_RISE, knock.y);
    if health.lose(damage) {
        kill_king_ant(commands, king, brain, animator);
    }
    true
}

/// The king's parts the knocks change.
type KnockQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut KingAntBrain,
        &'static mut Velocity,
        &'static mut Health,
        &'static EnemyModel,
    ),
    Without<Player>,
>;

/// The kick knocks a wet king on his butt.
///
/// Port of the `SKELETON_TYPE_KINGANT` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_king_ants(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut kings: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<KingAntBrain>>,
    mut players: Query<&mut Velocity, (With<Player>, Without<KingAntBrain>)>,
) {
    for kick in kicks.read() {
        let Ok((mut brain, mut velocity, mut health, model)) = kings.get_mut(kick.enemy) else {
            continue;
        };
        let knocked = knock_king_ant_on_butt(
            &mut commands,
            kick.enemy,
            &mut brain,
            animators.get_mut(model.0).ok().as_deref_mut(),
            &mut velocity,
            &mut health,
            kick.direction * KICK_KNOCK_SPEED,
            kick.damage,
        );
        if knocked && let Ok(mut player) = players.get_mut(kick.player) {
            **player *= PLAYER_SLOWDOWN;
        }
    }
}

/// A fast enough ball knocks a wet king on his butt; a dry king laughs it
/// off.
///
/// Port of `BallHitKingAnt` (original/src/Enemies/Enemy_KingAnt.c).
fn ball_hit_king_ants(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut kings: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<KingAntBrain>>,
    mut players: Query<&mut Velocity, (With<Player>, Without<KingAntBrain>)>,
) {
    for hit in hits.read() {
        let Ok((mut brain, mut velocity, mut health, model)) = kings.get_mut(hit.enemy) else {
            continue;
        };
        if !brain.is_wet() {
            // Sound: EFFECT_KINGLAUGH at the king, pitch kMiddleC, volume 2.0.
            continue;
        }
        if hit.ball_speed <= KNOCKDOWN_SPEED {
            continue;
        }
        let knocked = knock_king_ant_on_butt(
            &mut commands,
            hit.enemy,
            &mut brain,
            animators.get_mut(model.0).ok().as_deref_mut(),
            &mut velocity,
            &mut health,
            hit.ball_velocity.xz() * BALL_KNOCK_SHARE,
            BALL_DAMAGE,
        );
        if knocked && let Ok(mut player) = players.get_mut(hit.player) {
            **player *= PLAYER_SLOWDOWN;
        }
        // Sound: EFFECT_POUND at the player, pitch kMiddleC-2, volume 2.0.
    }
}

/// The king's health ran out: he dies.
///
/// Port of the `ENEMY_KIND_KINGANT` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_king_ants(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut kings: Query<(&mut KingAntBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator, Without<KingAntBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = kings.get_mut(kill.enemy) else {
            continue;
        };
        kill_king_ant(
            &mut commands,
            kill.enemy,
            &mut brain,
            animators.get_mut(model.0).ok().as_deref_mut(),
        );
    }
}

/// What the king starts doing after his move, given the player's distance
/// in x and z, or `None` to carry on. Port of the checks at the end of
/// `MoveKingAnt_Waiting` and `MoveKingAnt_Walk`.
fn next_state(brain: &KingAntBrain, distance: f32) -> Option<KingAntState> {
    match brain.state {
        KingAntState::Wait if distance < CHASE_DIST => Some(KingAntState::Walk),
        // He doesn't attack while wet.
        KingAntState::Walk if !brain.is_wet() && distance < ATTACK_DIST => {
            Some(KingAntState::Attack)
        }
        _ => None,
    }
}

/// Charges the staff for `dt` seconds. Returns whether it fires now
/// (`MoveKingAnt_Attack`).
fn charge_staff(brain: &mut KingAntBrain, dt: f32) -> bool {
    brain.staff_charge += dt;
    brain.staff_charge > STAFF_CHARGE_TIME
}

/// Turns toward `target` at [`TURN_SPEED`] for `dt` seconds
/// (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) {
    let (yaw, _) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
}

/// Falls under gravity, slowed by the friction if `slow`, and moves.
fn fall(body: &mut EnemyBodyItem, slow: bool, dt: f32) {
    if slow {
        apply_friction(
            &mut body.velocity,
            per_frame_friction(FRICTION_PER_FRAME),
            dt,
        );
    }
    body.velocity.y -= ENEMY_GRAVITY * dt;
    let velocity = **body.velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// Moves the king by his state.
///
/// Port of `MoveKingAnt`, `MoveKingAnt_Waiting`, `MoveKingAnt_Walk`,
/// `MoveKingAnt_Attack`, `MoveKingAnt_OnButt` and `MoveKingAnt_Death`
/// (original/src/Enemies/Enemy_KingAnt.c). `UpdateKingAnt`, which each of
/// them ends with, is [`update_king_ants`]. He goes for the nearest player.
#[allow(clippy::too_many_arguments)]
fn move_king_ants(
    mut commands: Commands,
    mut collision: EnemyCollision,
    mut kings: Query<(EnemyBody, &mut KingAntBrain, &KingStaff, &EnemyModel), Without<Player>>,
    mut animators: Query<&mut SkeletonAnimator, Without<KingAntBrain>>,
    players: Query<&Transform, (With<Player>, Without<KingAntBrain>)>,
    completed: Option<ResMut<AreaCompleted>>,
) {
    let dt = collision.dt();
    let mut completed = completed;
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut brain, staff, model) in &mut kings {
        let king = body.entity;
        let mut animator = animators.get_mut(model.0).ok();
        let coord = body.transform.translation;
        let player = nearest_player(coord, player_positions.iter().copied()).unwrap_or(coord);
        let mut mask = default_enemy_collision_mask();

        let state = brain.state;
        match state {
            KingAntState::Wait => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                fall(&mut body, false, dt);
            }
            KingAntState::Walk => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * WALK_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                fall(&mut body, false, dt);
            }
            KingAntState::Attack => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                fall(&mut body, true, dt);
            }
            KingAntState::OnButt => {
                let on_ground = body.ground.on_ground;
                fall(&mut body, on_ground, dt);
            }
            KingAntState::Death => {
                fall(&mut body, true, dt);
                mask = death_enemy_collision_mask();
            }
        }

        // Port of `DoEnemyCollisionDetect`. `KillKingAnt` never deletes, so
        // its effects can wait until the collision ends.
        let mut killed = false;
        collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        if killed {
            // The original carries on with the move of the state the king
            // was in, which can switch the dying king back to waiting or
            // walking (and have him shoot). Here his move ends, as the
            // flying bee's and the spider's do: an intentional difference.
            kill_king_ant(&mut commands, king, &mut brain, animator.as_deref_mut());
            continue;
        }

        let at = body.transform.translation;
        let distance = quick_distance(at.xz(), player.xz());
        match state {
            KingAntState::Wait | KingAntState::Walk => {
                if let Some(next) = next_state(&brain, distance) {
                    let rate = if next == KingAntState::Walk {
                        WALK_MORPH_RATE
                    } else {
                        brain.staff_charge = 0.0;
                        ATTACK_MORPH_RATE
                    };
                    brain.morph_state(animator.as_deref_mut(), next, rate);
                }
            }
            KingAntState::Attack => {
                if brain.is_wet() {
                    brain.morph_state(animator.as_deref_mut(), KingAntState::Walk, WALK_MORPH_RATE);
                } else if charge_staff(&mut brain, dt) {
                    staff::shoot_staff(&mut commands, staff, player);
                    brain.morph_state(
                        animator.as_deref_mut(),
                        KingAntState::Wait,
                        WAIT_AFTER_SHOT_MORPH_RATE,
                    );
                }
            }
            KingAntState::OnButt => {
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    brain.morph_state(
                        animator.as_deref_mut(),
                        KingAntState::Wait,
                        WAIT_AFTER_BUTT_MORPH_RATE,
                    );
                }
            }
            KingAntState::Death => {
                brain.death_timer -= dt;
                if brain.death_timer < 0.0
                    && let Some(completed) = completed.as_deref_mut()
                    && !**completed
                {
                    info!("The king ant is dead: area completed");
                    **completed = true;
                }
            }
        }
    }
}

/// What the king's update needs of his model.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, Option<&'static SkeletonRig>),
    (With<SkeletonAnimator>, Without<KingAntBrain>),
>;

/// The end of each of the king's moves: his staff goes to his hand and
/// burns, water puts his fire out, and while dry and alive his hair burns.
///
/// Port of `UpdateKingAnt` (original/src/Enemies/Enemy_KingAnt.c). As in
/// the original, his joints are where the last frame posed them.
#[allow(clippy::type_complexity)]
fn update_king_ants(
    time: Res<Time>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut kings: Query<(
        &Transform,
        &CollisionBoxes,
        &BoundingRadius,
        &mut KingAntBrain,
        &mut KingStaff,
        &EnemyModel,
    )>,
    models: ModelQuery,
    mut staffs: Query<
        &mut Transform,
        (
            With<Staff>,
            Without<KingAntBrain>,
            Without<SkeletonAnimator>,
        ),
    >,
    culling: EnemyCulling,
) {
    let dt = time.delta_secs();
    for (transform, boxes, radius, mut brain, mut staff, model) in &mut kings {
        let base = models.get(model.0).ok().and_then(|(model_transform, rig)| {
            Some((
                rig?,
                transform.compute_affine() * model_transform.compute_affine(),
            ))
        });

        if let Some((rig, base)) = base {
            staff::hold_staff(&mut staff, &mut staffs, rig, base, transform);
        }
        staff::burn_staff(&mut staff, &brain, &mut groups, &mut random, dt);

        // Water puts the fire out.
        if particle_hit(
            &groups,
            &boxes.0,
            transform.translation,
            ParticleFlags::EXTINGUISH,
        ) {
            if !brain.is_wet() {
                // Sound: EFFECT_SIZZLE at the king.
            }
            brain.wet_timer = WET_TIME;
        }

        if !brain.is_wet() && brain.state != KingAntState::Death {
            // Sound: EFFECT_KINGCRACKLE at the king, kept playing and
            // following him.
            if !culling.is_culled(transform.translation, **radius) {
                let head = base.and_then(|(rig, base)| {
                    joint_position(rig, HEAD_JOINT, HEAD_FLAME_OFFSET, base)
                });
                burn_hair(&mut brain, &mut groups, &mut random, dt, head);
            }
        } else {
            dry_off(&mut brain, dt);
            // Sound: stop the crackle.
        }
    }
}

/// Dries the king's fire for `dt` seconds (`UpdateKingAnt`).
fn dry_off(brain: &mut KingAntBrain, dt: f32) {
    brain.wet_timer = (brain.wet_timer - dt).max(0.0);
}

/// Adds a flame to the fire on the king's head now and then, at `head`.
/// Port of the flame in `UpdateKingAnt`
/// (original/src/Enemies/Enemy_KingAnt.c).
fn burn_hair(
    brain: &mut KingAntBrain,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    dt: f32,
    head: Option<Vec3>,
) {
    brain.head_flame_timer += dt;
    if brain.head_flame_timer <= HEAD_FLAME_INTERVAL {
        return;
    }
    if !brain.head_flame_group.is_some_and(|g| groups.is_valid(g)) {
        brain.head_flame_group = groups.new_group(HEAD_FLAME_GROUP);
    }
    let Some(group) = brain.head_flame_group else {
        return;
    };
    brain.head_flame_timer = 0.0;
    let Some(head) = head else {
        return;
    };
    let mut jitter = || {
        Vec3::new(
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
        )
    };
    let point = head + jitter() * HEAD_FLAME_SPREAD;
    let velocity = jitter() * HEAD_FLAME_SPEED + Vec3::Y * HEAD_FLAME_RISE;
    let scale = random.next_f32() + HEAD_FLAME_MIN_SCALE;
    // A full group just misses a flame, as in the original.
    groups.add_particle(group, point, velocity, scale, FULL_ALPHA);
}

/// Draws the burning parts of the king's model (eyebrows, hair and beard)
/// in their own colours, without the level's lights, once the rig is
/// spawned and lit.
///
/// Port of the null shading `LoadASkeleton` gives those meshes of the
/// king's skeleton (original/src/Skeleton/SkeletonObj.c).
#[allow(clippy::type_complexity)]
fn light_fiery_parts(
    mut commands: Commands,
    kings: Query<&EnemyModel, With<KingAntBrain>>,
    skeletons: Query<(&Skeleton, &Children)>,
    assets: Res<Assets<SkeletonAsset>>,
    mut meshes: Query<(&Mesh3d, &mut MeshMaterial3d<ObjectMaterial>), Without<FieryPart>>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    for model in &kings {
        let Ok((skeleton, children)) = skeletons.get(model.0) else {
            continue;
        };
        let Some(asset) = assets.get(&skeleton.0) else {
            continue;
        };
        let fiery: Vec<AssetId<Mesh>> = FIERY_PARTS
            .iter()
            .filter_map(|&i| asset.parts.get(i))
            .map(|part| part.mesh.id())
            .collect();
        for &child in children {
            let Ok((mesh, mut material)) = meshes.get_mut(child) else {
                continue;
            };
            if !fiery.contains(&mesh.id()) {
                continue;
            }
            let Some(mut unlit) = materials.get(&material.0).cloned() else {
                continue;
            };
            unlit.extension.lighting.lit = 0.0;
            material.0 = materials.add(unlit);
            commands.entity(child).insert(FieryPart);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::player::BallHitEnemy;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn the_king_walks_at_a_near_player_and_attacks_a_close_one_when_dry() {
        let mut brain = KingAntBrain::default();
        assert_eq!(next_state(&brain, 2000.0), None);
        assert_eq!(next_state(&brain, 1500.0), Some(KingAntState::Walk));
        brain.state = KingAntState::Walk;
        assert_eq!(next_state(&brain, 700.0), None);
        assert_eq!(next_state(&brain, 500.0), Some(KingAntState::Attack));
        brain.wet_timer = 1.0;
        assert_eq!(next_state(&brain, 500.0), None, "no attack while wet");
        brain.state = KingAntState::Attack;
        assert_eq!(next_state(&brain, 100.0), None);
    }

    #[test]
    fn the_staff_fires_after_two_seconds_of_charge() {
        let mut brain = KingAntBrain::default();
        let mut ticks = 0;
        while !charge_staff(&mut brain, DT) {
            ticks += 1;
            assert!(ticks < 1000);
        }
        assert_eq!(ticks, 120);
    }

    #[test]
    fn the_fire_stays_out_for_a_while() {
        let mut brain = KingAntBrain {
            wet_timer: WET_TIME,
            ..default()
        };
        dry_off(&mut brain, 4.0);
        assert!(brain.is_wet());
        dry_off(&mut brain, 2.0);
        assert!(!brain.is_wet());
        assert_eq!(brain.wet_timer, 0.0);
    }

    #[test]
    fn the_hair_burns_one_flame_at_a_time() {
        let mut brain = KingAntBrain::default();
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let head = Vec3::new(0.0, 300.0, 0.0);
        burn_hair(&mut brain, &mut groups, &mut random, 0.005, Some(head));
        assert_eq!(brain.head_flame_group, None, "not due yet");
        burn_hair(&mut brain, &mut groups, &mut random, 0.006, Some(head));
        let group = brain.head_flame_group.and_then(|g| groups.get(g));
        assert_eq!(group.map(|g| g.desc), Some(HEAD_FLAME_GROUP));
        let particles = group.map(|g| g.particles().to_vec()).unwrap_or_default();
        assert_eq!(particles.len(), 1);
        let offset = particles[0].position - head;
        assert!(offset.abs().cmple(HEAD_FLAME_SPREAD / 2.0).all());
        assert!(particles[0].velocity.y >= HEAD_FLAME_RISE - HEAD_FLAME_SPEED.y / 2.0);
    }

    /// A king with his model and a player, for the message handlers.
    fn world_with_king(brain: KingAntBrain) -> (World, Entity, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<BallHitEnemy>>();
        world.init_resource::<Messages<EnemyKicked>>();
        world.init_resource::<Messages<EnemyKilled>>();
        let model = world.spawn(SkeletonAnimator::default()).id();
        let king = world
            .spawn((
                brain,
                Transform::default(),
                EnemyModel(model),
                Velocity::default(),
                Health(KING_ANT_HEALTH),
            ))
            .id();
        let player = world
            .spawn((Player, Velocity(Vec3::new(100.0, 50.0, 0.0))))
            .id();
        (world, king, model, player)
    }

    fn ball_hit(player: Entity, enemy: Entity, speed: f32) -> BallHitEnemy {
        BallHitEnemy {
            player,
            enemy,
            ball_velocity: Vec3::new(0.0, 0.0, -speed),
            ball_speed: speed,
        }
    }

    fn brain(world: &World, king: Entity) -> KingAntBrain {
        world.get::<KingAntBrain>(king).copied().unwrap_or_default()
    }

    #[test]
    fn the_ball_does_nothing_to_a_dry_king() {
        let (mut world, king, _, player) = world_with_king(KingAntBrain::default());
        world.write_message(ball_hit(player, king, 2000.0));
        world
            .run_system_once(ball_hit_king_ants)
            .expect("the system runs");
        assert_eq!(brain(&world, king).state, KingAntState::Wait);
        assert_eq!(world.get::<Health>(king), Some(&Health(KING_ANT_HEALTH)));
    }

    #[test]
    fn a_fast_ball_knocks_a_wet_king_down_once() {
        let (mut world, king, model, player) = world_with_king(KingAntBrain {
            wet_timer: 1.0,
            ..default()
        });
        world.write_message(ball_hit(player, king, 1000.0));
        world
            .run_system_once(ball_hit_king_ants)
            .expect("the system runs");
        assert_eq!(brain(&world, king).state, KingAntState::Wait, "too slow");

        world.write_message(ball_hit(player, king, 2000.0));
        world.write_message(ball_hit(player, king, 2000.0));
        world
            .run_system_once(ball_hit_king_ants)
            .expect("the system runs");
        let b = brain(&world, king);
        assert_eq!(b.state, KingAntState::OnButt);
        assert_eq!(b.butt_timer, BUTT_TIME);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(KingAntState::OnButt.anim())
        );
        assert_eq!(
            world.get::<Velocity>(king).map(|v| v.0),
            Some(Vec3::new(0.0, KNOCK_RISE, -2000.0 * BALL_KNOCK_SHARE))
        );
        let health = world.get::<Health>(king).map(|h| h.0);
        assert_eq!(health, Some(KING_ANT_HEALTH - BALL_DAMAGE));
        let slowed = world.get::<Velocity>(player).map(|v| v.0);
        assert_eq!(slowed, Some(Vec3::new(100.0, 50.0, 0.0) * PLAYER_SLOWDOWN));
    }

    #[test]
    fn the_kick_knocks_a_wet_king_back_and_its_last_hurt_kills_him() {
        let (mut world, king, model, player) = world_with_king(KingAntBrain {
            wet_timer: 1.0,
            ..default()
        });
        world.entity_mut(king).insert(Health(0.1));
        world.write_message(EnemyKicked {
            player,
            enemy: king,
            direction: Vec2::new(1.0, 0.0),
            damage: 0.2,
        });
        world
            .run_system_once(kick_king_ants)
            .expect("the system runs");
        let b = brain(&world, king);
        assert_eq!(b.state, KingAntState::Death);
        assert_eq!(b.death_timer, DEATH_TIME);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(KingAntState::Death.anim())
        );
        assert_eq!(
            world.get::<Velocity>(king).map(|v| v.0),
            Some(Vec3::new(KICK_KNOCK_SPEED, KNOCK_RISE, 0.0))
        );
        assert_eq!(
            world.get::<CollisionLayers>(king).map(|l| l.memberships),
            Some(CollisionKind::Misc.into())
        );
    }

    #[test]
    fn a_killed_king_dies() {
        let (mut world, king, model, _) = world_with_king(KingAntBrain {
            state: KingAntState::Walk,
            ..default()
        });
        world.write_message(EnemyKilled {
            enemy: king,
            knock: Vec3::ZERO,
        });
        world
            .run_system_once(kill_hurt_king_ants)
            .expect("the system runs");
        let b = brain(&world, king);
        assert_eq!(b.state, KingAntState::Death);
        assert_eq!(b.death_timer, DEATH_TIME);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(KingAntState::Death.anim())
        );
    }

    #[test]
    fn the_king_is_on_his_level_and_the_pipes_spray_water() {
        use bugdom_formats::rsrc::ResourceFork;
        let path = bugdom_formats::original_data_dir().join("Terrain/AntKing.ter.rsrc");
        let fork = ResourceFork::open(&path).expect("terrain file");
        let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
        let kings = terrain
            .items
            .iter()
            .filter(|i| i.kind == kind::KING_ANT && i.params[3] == 0)
            .count();
        assert_eq!(kings, 1);
        let pipes = terrain
            .items
            .iter()
            .filter(|i| i.kind == kind::KING_WATER_PIPE)
            .count();
        assert!(pipes > 0);
    }
}
