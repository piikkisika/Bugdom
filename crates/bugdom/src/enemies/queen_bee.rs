//! The queen bee: the boss of the Queen Bee level. She waits at one of her
//! bases, turning to face the player, then spits a blob of honey that
//! grows and hatches a larva (or, once she has lost half her health, a
//! flying bee). Then she flies up to the midpoint of the way to her next
//! base, hovers there for a while and lands at the base. A fast ball or a
//! kick knocks her on her butt and hurts her; when her health runs out she
//! dies, and a few seconds later the level is completed.
//!
//! Port of original/src/Enemies/Enemy_QueenBee.c. The queen and her bases
//! are all map items of [`kind::QUEEN_BEE`]: the one whose `params[0]` is 0
//! spawns the queen (`AddEnemy_QueenBee`), and each one is a base whose
//! number is its `params[0]` (`FindQueenBases`). She is never deleted
//! ("always active"), so she has no `DespawnOutOfRange`. Her health shows
//! on the infobar's boss bar ([`BossHealthBar`]).
//!
//! The original keeps the bases and her flight's way points in globals
//! (`gQueenBase`, `gCurrentQueenBase`, `gMidPoint`, `gEndPoint`); here they
//! are on the queen, in [`QueenBases`] and [`QueenBeeBrain`].
//!
//! The kick: `DoBugKick` has a case for her, but she is not
//! `CTYPE_KICKABLE`, so the bug's kick never picks her. Her
//! [`EnemyKicked`] handler is ported all the same.

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyBodyItem, EnemyCollision, EnemyKicked,
    EnemyKilled, EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, apply_friction,
    death_enemy_collision_mask, default_enemy_collision_mask, move_enemy, nearest_player,
    per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionKind, SolidSides};
use crate::combat::{BossHealthBar, Health};
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroups, ParticleKind, ParticleTexture,
};
use crate::items::{
    AreaCompleted, DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItems,
    forget_terrain_item, kind,
};
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_of};
use crate::physics::Velocity;
use crate::player::Player;
use crate::skeleton::{
    AnimationFlags, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position,
};
use crate::terrain::{MAP_TO_WORLD, TerrainMap};

mod spit;

pub use spit::QueenSpit;

pub struct QueenBeePlugin;

impl Plugin for QueenBeePlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::QUEEN_BEE, add_queen_bee)
            .add_message::<spit::SpitShot>()
            .add_systems(
                FixedUpdate,
                (
                    kick_queen_bee.in_set(EnemySystems::Kicked),
                    // The player's collision reaches the queen before she
                    // moves, as in the original's frame.
                    (
                        ball_hit_queen_bee,
                        move_queen_bee,
                        spit::shoot_spit,
                        spit::move_queen_spit,
                    )
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_queen_bee.in_set(EnemySystems::Killed),
                ),
            );
    }
}

/// `QUEENBEE_HEALTH` (original/src/Headers/enemy.h): "LOTS of health!".
pub const QUEEN_BEE_HEALTH: f32 = 7.0;
/// `QUEENBEE_SCALE`
const QUEEN_BEE_SCALE: f32 = 1.5;
/// Her origin is this far below her feet (`QUEENBEE_FOOT_OFFSET`); being
/// negative, it lifts her.
const QUEEN_BEE_FOOT_OFFSET: f32 = -90.0;
/// The top of her collision box, in units above her origin.
const QUEEN_BEE_HEAD_OFFSET: f32 = 100.0;
/// Half her collision box's width and depth, in units.
const QUEEN_BEE_HALF_WIDTH: f32 = 130.0;
/// Shadow size (`AttachShadowToObject(newObj, 13, 13, false)`).
const QUEEN_BEE_SHADOW_SCALE: f32 = 13.0;

/// How fast she turns to face the player, in radians per second
/// (`QUEENBEE_TURN_SPEED`).
const TURN_SPEED: f32 = 2.0;
/// Seconds she waits before her first spit (`WaitTimer = 3` in
/// `AddEnemy_QueenBee`).
const FIRST_WAIT_TIME: f32 = 3.0;
/// Seconds she spends spitting before she flies off (`SpitTimer = 2`).
const SPIT_TIME: f32 = 2.0;
/// Seconds she waits after landing (`WaitTimer = 1.5`).
const LANDED_WAIT_TIME: f32 = 1.5;
/// Seconds she hovers at the midpoint (`WaitTimer = 3`).
const HOVER_TIME: f32 = 3.0;
/// How far above the floor the midpoint of her flight is, in units
/// (`QUEENBEE_HOVER_HEIGHT`).
const HOVER_HEIGHT: f32 = 400.0;
/// Her flying speed, horizontally and climbing, in units per second.
const FLY_SPEED: f32 = 400.0;
/// How close to the midpoint she must come to hover there, in units.
const MIDPOINT_REACHED_DIST: f32 = 150.0;
/// How close to her base she must come to land on it, in units.
const BASE_REACHED_DIST: f32 = 100.0;
/// Friction while she hovers, per frame at 60 fps.
const HOVER_FRICTION_PER_FRAME: f32 = 5.0;

/// How fast the ball must go to knock her down, in units per second
/// (`QUEENBEE_KNOCKDOWN_SPEED`).
const KNOCKDOWN_SPEED: f32 = 1400.0;
/// How much of the ball's horizontal velocity she takes (`BallHitQueenBee`).
const BALL_KNOCK_SHARE: f32 = 0.3;
/// What the ball does to her health.
const BALL_DAMAGE: f32 = 0.7;
/// How fast the kick sends her along, in units per second (`DoBugKick`).
const KICK_KNOCK_SPEED: f32 = 300.0;
/// How fast a knock sends her up, in units per second
/// (`KnockQueenBeeOnButt`).
const KNOCK_RISE: f32 = 600.0;
/// How much of its velocity the player keeps after knocking her down
/// (`gDelta *= .2` in `KnockQueenBeeOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;
/// Seconds she sits on her butt (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// Friction while she is on her butt or dead on the ground, per frame at
/// 60 fps.
const BUTT_FRICTION_PER_FRAME: f32 = 60.0;
/// Seconds from her death to the end of the level (`DeathTimer = 4`).
const DEATH_TIME: f32 = 4.0;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const SPIT_MORPH_RATE: f32 = 6.0;
const FLY_MORPH_RATE: f32 = 6.0;
const LAND_MORPH_RATE: f32 = 5.0;
const BUTT_MORPH_RATE: f32 = 7.0;
const GET_UP_MORPH_RATE: f32 = 6.0;
const DEATH_MORPH_RATE: f32 = 8.0;

/// The head's joint (`QUEENBEE_JOINT_HEAD`), which spits.
const HEAD_JOINT: usize = 2;
/// Where the spit leaves the head, in the head joint's space.
const MOUTH_OFFSET: Vec3 = Vec3::new(0.0, -25.0, -55.0);
/// The animation flag the spitting animation sets when the spit should
/// leave (`SpitNowFlag`, `Flag[0]`).
const SPIT_NOW_FLAG: usize = 0;
/// `MAX_QUEEN_BASES`
const MAX_QUEEN_BASES: usize = 15;

/// The sparks a knock makes (`KnockQueenBeeOnButt`).
const KNOCK_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::BOUNCE,
    gravity: 500.0,
    magnetism: 0.0,
    base_scale: 20.0,
    decay_rate: 0.9,
    fade_rate: 0.0,
    texture: ParticleTexture::YellowBall,
};
const KNOCK_SPARKS: usize = 35;
/// The knock sparks' largest speed in each direction, in units per second
/// (the whole width).
const KNOCK_SPARK_SPEED: f32 = 1000.0;
/// The sparks of her death (`KillQueenBee`).
const DEATH_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    gravity: 400.0,
    decay_rate: 0.8,
    ..KNOCK_SPARK_GROUP
};
const DEATH_SPARKS: usize = 60;
const DEATH_SPARK_SPEED: f32 = 1400.0;
/// The smallest spark's scale; they are up to 1 bigger.
const SPARK_MIN_SCALE: f32 = 1.0;

/// How her flight goes (`QUEENBEE_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FlyMode {
    /// Up to the midpoint of the way to the next base.
    #[default]
    ToMidpoint,
    /// Hovering there "while enemies do their thing".
    Hover,
    /// Over to the base, to land on it.
    Land,
}

/// What the queen is doing. The original dispatches on her animation
/// (`myMoveTable[AnimNum]`); each state plays the animation of the same
/// name (`QUEENBEE_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueenBeeState {
    #[default]
    Wait,
    Spitting,
    Fly(FlyMode),
    OnButt,
    Death,
}

impl QueenBeeState {
    /// The state's animation (`QUEENBEE_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Wait => 0,
            Self::Spitting => 1,
            Self::Fly(_) => 2,
            Self::OnButt => 3,
            Self::Death => 4,
        }
    }
}

/// The queen's own state, on her root entity.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct QueenBeeBrain {
    pub state: QueenBeeState,
    /// Seconds left to wait, spit or hover. The original's `WaitTimer` and
    /// `SpitTimer` are the same slot, and the states rely on it: getting
    /// off her butt sets it to 0, so she spits at once.
    pub timer: f32,
    /// Seconds left on her butt (`ButtTimer`).
    pub butt_timer: f32,
    /// Seconds left until the level ends, once dead (`DeathTimer`).
    pub death_timer: f32,
    /// She has yet to spit this time (`CanSpit`).
    pub can_spit: bool,
    /// Where she flies up to (`gMidPoint`).
    pub midpoint: Vec3,
    /// The base she lands on, in x and z (`gEndPoint`).
    pub end_point: Vec2,
}

impl Default for QueenBeeBrain {
    fn default() -> Self {
        Self {
            state: QueenBeeState::Wait,
            timer: FIRST_WAIT_TIME,
            butt_timer: 0.0,
            death_timer: 0.0,
            can_spit: false,
            midpoint: Vec3::ZERO,
            end_point: Vec2::ZERO,
        }
    }
}

impl QueenBeeBrain {
    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(
        &mut self,
        animator: Option<&mut SkeletonAnimator>,
        state: QueenBeeState,
        rate: f32,
    ) {
        self.state = state;
        if let Some(animator) = animator {
            animator.morph_to(state.anim(), rate);
        }
    }

    fn is_dead(&self) -> bool {
        self.state == QueenBeeState::Death
    }
}

/// One of the queen's bases: a [`kind::QUEEN_BEE`] map item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueenBase {
    /// World x and z.
    pub position: Vec2,
    /// Its number, the order she visits it in (`gQueenBaseID`).
    pub id: u8,
}

/// The queen's bases and the one she is at (`gQueenBase`, `gQueenBaseID`,
/// `gCurrentQueenBase`).
#[derive(Component, Debug, Clone, PartialEq, Default)]
pub struct QueenBases {
    pub bases: Vec<QueenBase>,
    /// The number of the base she is at or flying to.
    pub current: usize,
}

impl QueenBases {
    /// The bases among the level's items, in the items' order, up to
    /// [`MAX_QUEEN_BASES`].
    ///
    /// Port of `FindQueenBases` (original/src/Enemies/Enemy_QueenBee.c).
    pub fn find(items: &TerrainItems) -> Self {
        let bases = items
            .items
            .iter()
            .filter(|item| item.kind == kind::QUEEN_BEE)
            .take(MAX_QUEEN_BASES)
            .map(|item| QueenBase {
                position: Vec2::new(f32::from(item.x), f32::from(item.z)) * MAP_TO_WORLD,
                id: item.params[0],
            })
            .collect();
        Self { bases, current: 0 }
    }

    /// Moves on to the next base by number, wrapping round, and returns
    /// where it is.
    ///
    /// Port of the base choice in `MoveQueenBee_Spitting`. When no base has
    /// the next number, the original reads past the end of its list; here
    /// there is no next base, and she lands where she is.
    pub fn advance(&mut self) -> Option<Vec2> {
        self.current += 1;
        if self.current >= self.bases.len() {
            self.current = 0;
        }
        self.bases
            .iter()
            .find(|base| usize::from(base.id) == self.current)
            .map(|base| base.position)
    }
}

/// Port of `AddEnemy_QueenBee` (original/src/Enemies/Enemy_QueenBee.c).
/// Only the item numbered 0 makes the queen, at the first base in the item
/// list; the others are only bases, and count as spawned. There is no Add
/// guard.
fn add_queen_bee(
    In(spawn): In<ItemSpawn>,
    items: Res<TerrainItems>,
    mut enemies: EnemySpawner,
) -> bool {
    if spawn.params[0] != 0 {
        return true;
    }
    let bases = QueenBases::find(&items);
    let Some(first) = bases.bases.first().map(|b| b.position) else {
        // The original stops the game ("No bases found"); the item itself
        // is a base, so this can't happen.
        error!("The queen bee has no bases");
        return false;
    };
    let Some(queen) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::QueenBee,
            SkeletonType::QueenBee,
            first,
            QUEEN_BEE_SCALE,
        )
        .from_item(spawn.index)
        .foot_offset(QUEEN_BEE_FOOT_OFFSET)
        .collision_box(CollisionBox::new(
            QUEEN_BEE_HEAD_OFFSET,
            QUEEN_BEE_FOOT_OFFSET,
            -QUEEN_BEE_HALF_WIDTH,
            QUEEN_BEE_HALF_WIDTH,
            QUEEN_BEE_HALF_WIDTH,
            -QUEEN_BEE_HALF_WIDTH,
        ))
        .health(QUEEN_BEE_HEALTH)
        .damage(0.0)
        .anim(QueenBeeState::Wait.anim())
        .shadow(QUEEN_BEE_SHADOW_SCALE),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(queen)
        .remove::<DespawnOutOfRange>()
        .insert((
            QueenBeeBrain::default(),
            bases,
            BossHealthBar {
                full: QUEEN_BEE_HEALTH,
            },
        ));
    true
}

/// The yellow sparks of a knock or of her death, at `at`.
fn sparks(
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    desc: ParticleGroupDesc,
    count: usize,
    speed: f32,
    at: Vec3,
) {
    let Some(group) = groups.new_group(desc) else {
        return;
    };
    for _ in 0..count {
        let mut spread = || (random.next_f32() - 0.5) * speed;
        let velocity = Vec3::new(spread(), spread(), spread());
        let scale = random.next_f32() + SPARK_MIN_SCALE;
        groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
    }
}

/// What a knock or a death changes on the queen, for [`knock_queen_bee_on_butt`]
/// and [`kill_queen_bee`].
struct QueenParts<'a> {
    entity: Entity,
    brain: &'a mut QueenBeeBrain,
    animator: Option<&'a mut SkeletonAnimator>,
    at: Vec3,
}

/// Kills the queen: she never comes back, becomes a plain obstacle and
/// plays her death, and the level ends [`DEATH_TIME`] later. Repeats are
/// ignored.
///
/// Port of `KillQueenBee` (original/src/Enemies/Enemy_QueenBee.c).
fn kill_queen_bee(
    commands: &mut Commands,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    queen: QueenParts,
) {
    if queen.brain.is_dead() {
        return;
    }
    // Sound: stop her EFFECT_BUZZ.
    let mut entity = commands.entity(queen.entity);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    queen
        .brain
        .morph_state(queen.animator, QueenBeeState::Death, DEATH_MORPH_RATE);
    sparks(
        groups,
        random,
        DEATH_SPARK_GROUP,
        DEATH_SPARKS,
        DEATH_SPARK_SPEED,
        queen.at,
    );
    queen.brain.death_timer = DEATH_TIME;
}

/// Knocks the queen on her butt, moving at `knock` in x and z, and hurts
/// her by `damage`, killing her if that was the last of her health.
/// Returns false if she was on her butt already, and nothing happened.
///
/// Port of `KnockQueenBeeOnButt` (original/src/Enemies/Enemy_QueenBee.c),
/// but for slowing the player down, which the caller does with the player
/// it knows. A dead queen can't be knocked: the original's ball and kick
/// only reach enemies, which she no longer is.
#[allow(clippy::too_many_arguments)]
fn knock_queen_bee_on_butt(
    commands: &mut Commands,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    mut queen: QueenParts,
    velocity: &mut Velocity,
    health: &mut Health,
    knock: Vec2,
    damage: f32,
) -> bool {
    if matches!(
        queen.brain.state,
        QueenBeeState::OnButt | QueenBeeState::Death
    ) {
        return false;
    }
    queen.brain.morph_state(
        queen.animator.as_deref_mut(),
        QueenBeeState::OnButt,
        BUTT_MORPH_RATE,
    );
    queen.brain.butt_timer = BUTT_TIME;
    **velocity = Vec3::new(knock.x, KNOCK_RISE, knock.y);
    sparks(
        groups,
        random,
        KNOCK_SPARK_GROUP,
        KNOCK_SPARKS,
        KNOCK_SPARK_SPEED,
        queen.at,
    );
    if health.lose(damage) {
        kill_queen_bee(commands, groups, random, queen);
    }
    true
}

/// The queen's parts that a knock reads and writes.
type KnockQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static mut QueenBeeBrain,
        &'static mut Velocity,
        &'static mut Health,
        &'static EnemyModel,
    ),
    Without<Player>,
>;

/// Shared by the ball and the kick: knocks the queen and slows the player.
#[allow(clippy::too_many_arguments)]
fn knock_and_slow_player(
    commands: &mut Commands,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    queens: &mut KnockQuery,
    animators: &mut Query<&mut SkeletonAnimator, Without<QueenBeeBrain>>,
    players: &mut Query<&mut Velocity, (With<Player>, Without<QueenBeeBrain>)>,
    queen: Entity,
    player: Entity,
    knock: Vec2,
    damage: f32,
) {
    let Ok((transform, mut brain, mut velocity, mut health, model)) = queens.get_mut(queen) else {
        return;
    };
    let parts = QueenParts {
        entity: queen,
        brain: &mut brain,
        animator: animators.get_mut(model.0).ok().map(|a| a.into_inner()),
        at: transform.translation,
    };
    let knocked = knock_queen_bee_on_butt(
        commands,
        groups,
        random,
        parts,
        &mut velocity,
        &mut health,
        knock,
        damage,
    );
    if knocked && let Ok(mut player) = players.get_mut(player) {
        **player *= PLAYER_SLOWDOWN;
    }
}

/// A fast enough ball knocks the queen on her butt.
///
/// Port of `BallHitQueenBee` (original/src/Enemies/Enemy_QueenBee.c).
#[allow(clippy::too_many_arguments)]
fn ball_hit_queen_bee(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut queens: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<QueenBeeBrain>>,
    mut players: Query<&mut Velocity, (With<Player>, Without<QueenBeeBrain>)>,
) {
    for hit in hits.read() {
        if hit.ball_speed <= KNOCKDOWN_SPEED || !queens.contains(hit.enemy) {
            continue;
        }
        knock_and_slow_player(
            &mut commands,
            &mut groups,
            &mut random,
            &mut queens,
            &mut animators,
            &mut players,
            hit.enemy,
            hit.player,
            hit.ball_velocity.xz() * BALL_KNOCK_SHARE,
            BALL_DAMAGE,
        );
        // Sound: EFFECT_POUND at the player, pitch kMiddleC+2, volume 2.0.
    }
}

/// The kick knocks the queen on her butt.
///
/// Port of the `SKELETON_TYPE_QUEENBEE` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c); see the module docs.
#[allow(clippy::too_many_arguments)]
fn kick_queen_bee(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut queens: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<QueenBeeBrain>>,
    mut players: Query<&mut Velocity, (With<Player>, Without<QueenBeeBrain>)>,
) {
    for kick in kicks.read() {
        if !queens.contains(kick.enemy) {
            continue;
        }
        knock_and_slow_player(
            &mut commands,
            &mut groups,
            &mut random,
            &mut queens,
            &mut animators,
            &mut players,
            kick.enemy,
            kick.player,
            kick.direction * KICK_KNOCK_SPEED,
            kick.damage,
        );
    }
}

/// The queen dies when her health runs out.
///
/// Port of the `ENEMY_KIND_QUEENBEE` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_queen_bee(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut queens: Query<(&Transform, &mut QueenBeeBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator, Without<QueenBeeBrain>>,
) {
    for kill in killed.read() {
        let Ok((transform, mut brain, model)) = queens.get_mut(kill.enemy) else {
            continue;
        };
        let parts = QueenParts {
            entity: kill.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok().map(|a| a.into_inner()),
            at: transform.translation,
        };
        kill_queen_bee(&mut commands, &mut groups, &mut random, parts);
    }
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

/// Falls under gravity and moves (`gDelta.y -= ENEMY_GRAVITY`,
/// `MoveEnemy`), slowed by `friction` (per frame at 60 fps) if on the
/// ground.
fn fall(body: &mut EnemyBodyItem, friction: Option<f32>, dt: f32) {
    if let Some(friction) = friction
        && body.ground.on_ground
    {
        apply_friction(&mut body.velocity, per_frame_friction(friction), dt);
    }
    body.velocity.y -= ENEMY_GRAVITY * dt;
    let velocity = **body.velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// The horizontal velocity that flies from `from` toward `to` at
/// [`FLY_SPEED`] (`FastNormalizeVector2D`).
fn fly_toward(from: Vec2, to: Vec2) -> Vec2 {
    (to - from).normalize_or_zero() * FLY_SPEED
}

/// Where she flies up to on the way from `from` to the base at `base`: the
/// midpoint in x and z, [`HOVER_HEIGHT`] above the floor there.
fn midpoint(from: Vec2, base: Vec2, floor_height: impl Fn(Vec2) -> f32) -> Vec3 {
    let mid = (from + base) * 0.5;
    Vec3::new(mid.x, floor_height(mid) + HOVER_HEIGHT, mid.y)
}

/// One step of her flight (`MoveQueenBee_Fly`, before the collision):
/// sets the velocity, moves, and changes the mode when she gets there.
/// Returns whether hitting something solid should make her land
/// (`checkSolid`).
fn fly(
    brain: &mut QueenBeeBrain,
    mode: FlyMode,
    coord: &mut Vec3,
    velocity: &mut Vec3,
    animator: Option<&mut SkeletonAnimator>,
    dt: f32,
) -> bool {
    let mut check_solid = false;
    match mode {
        FlyMode::ToMidpoint => {
            let aim = fly_toward(coord.xz(), brain.midpoint.xz());
            velocity.x = aim.x;
            velocity.z = aim.y;
            if coord.y < brain.midpoint.y {
                velocity.y = FLY_SPEED;
            } else {
                velocity.y = 0.0;
                check_solid = true;
            }
            move_enemy(coord, *velocity, dt);
            if quick_distance(coord.xz(), brain.midpoint.xz()) < MIDPOINT_REACHED_DIST {
                brain.state = QueenBeeState::Fly(FlyMode::Hover);
                brain.timer = HOVER_TIME;
            }
        }
        FlyMode::Hover => {
            apply_friction(velocity, per_frame_friction(HOVER_FRICTION_PER_FRAME), dt);
            move_enemy(coord, *velocity, dt);
            check_solid = true;
            brain.timer -= dt;
            if brain.timer <= 0.0 {
                brain.state = QueenBeeState::Fly(FlyMode::Land);
            }
        }
        FlyMode::Land => {
            // The climb speed is left as it was.
            let aim = fly_toward(coord.xz(), brain.end_point);
            velocity.x = aim.x;
            velocity.z = aim.y;
            move_enemy(coord, *velocity, dt);
            if quick_distance(coord.xz(), brain.end_point) < BASE_REACHED_DIST {
                *velocity = Vec3::ZERO;
                brain.morph_state(animator, QueenBeeState::Wait, LAND_MORPH_RATE);
                brain.timer = LANDED_WAIT_TIME;
            }
        }
    }
    check_solid
}

/// The parts of her model that her move reads.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SkeletonAnimator,
        &'static mut AnimationFlags,
        Option<&'static SkeletonRig>,
        &'static Transform,
    ),
    Without<QueenBeeBrain>,
>;

/// Moves the queen by her state.
///
/// Port of `MoveQueenBee`, `MoveQueenBee_Waiting`, `MoveQueenBee_Spitting`,
/// `MoveQueenBee_Fly`, `MoveQueenBee_OnButt`, `MoveQueenBee_Death` and
/// `UpdateQueenBee` (original/src/Enemies/Enemy_QueenBee.c). She faces the
/// nearest player. The spit she shoots is made by [`spit::shoot_spit`].
///
/// One difference: when a hurt in her collision kills her, the original
/// carries on with the rest of the state's move, which can start her
/// spitting, flying or standing up again in place of her death if its
/// timer runs out in that frame. Here her move ends with her death.
#[allow(clippy::too_many_arguments)]
fn move_queen_bee(
    mut collision: EnemyCollision,
    map: Res<TerrainMap>,
    mut completed: ResMut<AreaCompleted>,
    mut killed_messages: MessageWriter<EnemyKilled>,
    mut queens: Query<
        (EnemyBody, &mut QueenBeeBrain, &mut QueenBases, &EnemyModel),
        Without<Player>,
    >,
    mut models: ModelQuery,
    players: Query<&Transform, (With<Player>, Without<QueenBeeBrain>)>,
    mut spits: MessageWriter<spit::SpitShot>,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut brain, mut bases, model) in &mut queens {
        let queen = body.entity;
        let Ok((mut animator, mut flags, rig, model_transform)) = models.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let player = nearest_player(coord, player_positions.iter().copied()).unwrap_or(coord);
        let mut mask = default_enemy_collision_mask();
        let mut check_solid = false;

        let state = brain.state;
        match state {
            QueenBeeState::Wait => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                fall(&mut body, None, dt);
            }
            QueenBeeState::Spitting => fall(&mut body, None, dt),
            QueenBeeState::Fly(mode) => {
                turn_toward_target(&mut body.transform, player.xz(), dt);
                let mut at = body.transform.translation;
                let mut velocity = **body.velocity;
                check_solid = fly(
                    &mut brain,
                    mode,
                    &mut at,
                    &mut velocity,
                    Some(&mut animator),
                    dt,
                );
                body.transform.translation = at;
                **body.velocity = velocity;
            }
            QueenBeeState::OnButt => fall(&mut body, Some(BUTT_FRICTION_PER_FRAME), dt),
            QueenBeeState::Death => {
                brain.death_timer -= dt;
                if brain.death_timer <= 0.0 {
                    **completed = true;
                }
                fall(&mut body, Some(BUTT_FRICTION_PER_FRAME), dt);
                mask = death_enemy_collision_mask();
            }
        }

        // Port of `DoEnemyCollisionDetect`. `KillQueenBee` never deletes,
        // so its effects can wait until the collision ends.
        let mut killed = false;
        let contact = collision.collide(&mut body, mask, &mut |_, _| {
            killed = true;
            false
        });
        if killed && !brain.is_dead() {
            // The collision can't make the death's sparks while it reads
            // the particles, so `KillQueenBee` runs in
            // [`kill_hurt_queen_bee`], later in the same tick.
            killed_messages.write(EnemyKilled {
                enemy: queen,
                knock: Vec3::ZERO,
            });
            continue;
        }

        match state {
            QueenBeeState::Wait => {
                brain.timer -= dt;
                if brain.timer <= 0.0 {
                    brain.morph_state(
                        Some(&mut animator),
                        QueenBeeState::Spitting,
                        SPIT_MORPH_RATE,
                    );
                    brain.timer = SPIT_TIME;
                    flags.0[SPIT_NOW_FLAG] = false;
                    brain.can_spit = true;
                }
            }
            QueenBeeState::Spitting => {
                if brain.can_spit && flags.0[SPIT_NOW_FLAG] {
                    flags.0[SPIT_NOW_FLAG] = false;
                    brain.can_spit = false;
                    // `ShootSpit`: from the mouth, as the head was last
                    // drawn.
                    let base = Affine3A::from_rotation_translation(
                        body.transform.rotation,
                        body.transform.translation,
                    ) * model_transform.compute_affine();
                    if let Some(mouth) =
                        rig.and_then(|rig| joint_position(rig, HEAD_JOINT, MOUTH_OFFSET, base))
                    {
                        spits.write(spit::SpitShot {
                            at: mouth,
                            yaw: yaw_of(body.transform.rotation),
                        });
                    }
                }
                brain.timer -= dt;
                if brain.timer <= 0.0 {
                    let at = body.transform.translation.xz();
                    let base = bases.advance().unwrap_or_else(|| {
                        warn!("The queen bee has no base number {}", bases.current);
                        at
                    });
                    brain.end_point = base;
                    brain.midpoint = midpoint(at, base, |p| map.floor_height(p.x, p.y));
                    brain.morph_state(
                        Some(&mut animator),
                        QueenBeeState::Fly(FlyMode::ToMidpoint),
                        FLY_MORPH_RATE,
                    );
                }
            }
            QueenBeeState::Fly(_) => {
                // If she hit something solid while flying, she lands there.
                if check_solid && contact.boxes.sides != SolidSides::NONE {
                    brain.state = QueenBeeState::Fly(FlyMode::Land);
                    brain.timer = 0.0;
                    brain.end_point = body.transform.translation.xz();
                }
            }
            QueenBeeState::OnButt => {
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    brain.morph_state(Some(&mut animator), QueenBeeState::Wait, GET_UP_MORPH_RATE);
                    brain.timer = 0.0;
                }
            }
            QueenBeeState::Death => {}
        }
        // Sound: EFFECT_BUZZ at the queen, pitch kMiddleC-3, volume 3.0,
        // while she flies (`UpdateQueenBee`); stop it otherwise.
    }
}

#[cfg(test)]
mod tests;
