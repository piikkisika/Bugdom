//! The spider: it hangs out of sight high up until the player comes near,
//! drops on its thread, then walks at the player and spits web. A player
//! caught in the web is pounced on. Knocked down, it falls on its butt;
//! killed, it collapses.
//!
//! Port of original/src/Enemies/Enemy_Spider.c. Spiders are both map
//! items (`AddEnemy_Spider`) and spline items (`PrimeEnemy_Spider`), under
//! [`kind::SPIDER`]. The thread it drops on is in [`thread`]; the web
//! bullet and the sphere that traps the player are in [`web`].
//!
//! The spider answers [`EnemyKicked`] and [`BallHitEnemy`] (both knock it
//! on its butt) and [`EnemyKilled`]. Once dropped it is spiked, which the
//! player's collision handles; it isn't boppable.

mod thread;
mod web;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled,
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, KICK_SPEED, apply_friction,
    death_enemy_collision_mask, default_enemy_collision_mask, detach_enemy_from_spline, move_enemy,
    per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionKind, SolidSides};
use crate::combat::Health;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of};
use crate::objects::{ModelFile, ModelRef, ModelSpawner};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{BugState, Player, PlayerForm, PlayerSystems};
use crate::skeleton::{
    AnimationFlags, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position,
};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::AppState;
use crate::terrain::TerrainMap;

use thread::{SpiderThread, THREAD_YOFF};
pub use web::{WebBullet, WebSphere};
use web::{WebModels, is_webbed};

pub struct SpiderPlugin;

impl Plugin for SpiderPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::SPIDER, add_spider)
            .register_spline_item_kind(kind::SPIDER, prime_spider)
            .add_plugins(thread::plugin)
            .add_systems(
                FixedUpdate,
                (
                    kick_spiders.in_set(EnemySystems::Kicked),
                    // The player's collision reaches the spiders and sets
                    // off the web before they move, as in the original's
                    // frame.
                    (
                        ball_hit_spiders,
                        web::web_bullet_hits,
                        move_spiders,
                        web::move_web_bullets,
                    )
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_spiders.in_set(EnemySystems::Killed),
                    move_spiders_on_spline.in_set(SplineSystems::Move),
                    web::move_web_spheres
                        .after(PlayerSystems::Hold)
                        .run_if(in_state(AppState::InGame)),
                ),
            )
            .add_systems(
                Update,
                web::apply_web_glow.run_if(in_state(AppState::InGame)),
            );
    }
}

/// The most spiders counted at once (`MAX_SPIDER`).
const MAX_SPIDERS: usize = 5;
/// `SPIDER_SCALE`
pub const SPIDER_SCALE: f32 = 0.9;
const SPIDER_HEALTH: f32 = 0.3;
/// What touching a spider does to the player (`SPIDER_DAMAGE`).
const SPIDER_DAMAGE: f32 = 0.2;
/// How far above the floor a spider from a map item waits, in units
/// (`SPIDER_START_YOFF`).
pub const SPIDER_START_YOFF: f32 = 1000.0;
/// How far the spider's origin is above its feet (`SPIDER_FOOT_OFFSET`).
const SPIDER_FOOT_OFFSET: f32 = 55.0;
/// The spider's collision box (`SetObjectCollisionBounds(newObj, 40,
/// -SPIDER_FOOT_OFFSET, -90, 90, 90, -90)`).
const SPIDER_BOX: CollisionBox =
    CollisionBox::new(40.0, -SPIDER_FOOT_OFFSET, -90.0, 90.0, 90.0, -90.0);
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const SPIDER_SHADOW_SCALE: f32 = 8.0;

/// How close the player must come, in units, for a waiting spider to drop
/// (`SPIDER_DROP_DIST`).
const SPIDER_DROP_DIST: f32 = 700.0;
/// How fast a dropping spider falls for each unit it is above the ground,
/// per second, on top of [`DROP_MIN_SPEED`]: the drop slows as it nears the
/// ground.
const DROP_SPEED_PER_UNIT: f32 = 1.7;
/// The least speed a dropping spider falls at, in units per second.
const DROP_MIN_SPEED: f32 = 100.0;
/// How close the player must be, in units, for a spider to spit or pounce,
/// and for one on a spline to leave it (`SPIDER_ATTACK_DIST`).
const SPIDER_ATTACK_DIST: f32 = 500.0;
/// How well aimed at the player a spider must be to spit, in radians.
const SPIT_AIM: f32 = 1.0;
/// How fast a pouncing spider leaps up, in units per second
/// (`SPIDER_JUMPDY`).
const SPIDER_JUMPDY: f32 = 1100.0;
/// How fast a spider turns, in radians per second (`SPIDER_TURN_SPEED`).
const SPIDER_TURN_SPEED: f32 = 0.9;
/// Walking speed, in units per second (`SPIDER_WALK_SPEED`).
const SPIDER_WALK_SPEED: f32 = 300.0;
/// The walk animation's speed (`SPIDER_WALK_SPEED * .004`).
const SPIDER_WALK_ANIM_SPEED: f32 = SPIDER_WALK_SPEED * 0.004;
/// How fast the ball must go to knock a spider down, in units per second
/// (`SPIDER_KNOCKDOWN_SPEED`).
const SPIDER_KNOCKDOWN_SPEED: f32 = 1400.0;
/// Speed along a spline, in baked points per second
/// (`SPIDER_SPLINE_SPEED`).
const SPIDER_SPLINE_SPEED: f32 = 45.0;
/// The walk animation's speed on a spline.
const SPIDER_SPLINE_ANIM_SPEED: f32 = 0.9;

/// How long a spider sits on its butt, in seconds (`ButtTimer = 2.0`).
const BUTT_TIME: f32 = 2.0;
/// Friction on a spider on its butt on the ground, per frame at 60 fps.
const BUTT_FRICTION_PER_FRAME: f32 = 140.0;
/// Friction while getting up and while dead, per frame at 60 fps.
const GET_UP_FRICTION_PER_FRAME: f32 = 60.0;
/// How far from the player a dead spider must be, as well as out of view,
/// to be deleted, in units.
const DEAD_DELETE_DIST: f32 = 600.0;

/// How much of the ball's velocity a knocked-down spider takes
/// (`BallHitSpider`).
const BALL_KNOCK_SHARE: f32 = 0.8;
/// How fast the ball sends a spider up, on top of its share of the ball's
/// own rise, in units per second.
const BALL_KNOCK_RISE: f32 = 250.0;
/// What the ball does to a spider's health.
const BALL_DAMAGE: f32 = 0.6;
/// How much of its velocity the player keeps after knocking a spider down
/// (`gDelta *= .2` in `KnockSpiderOnButt`).
const PLAYER_SLOWDOWN: f32 = 0.2;

/// How much of each blend happens per second (`MorphToSkeletonAnim`).
const WALK_MORPH_RATE: f32 = 5.0;
const SPIT_MORPH_RATE: f32 = 2.0;
const JUMP_MORPH_RATE: f32 = 5.0;
const FALL_ON_BUTT_MORPH_RATE: f32 = 9.0;

/// The animation flag the spit animation sets when the web should fly
/// (`ShootWeb`, `Flag[0]`).
const SHOOT_WEB_FLAG: usize = 0;
/// The joint the web comes from (the head), and the mouth in its space
/// (`FindCoordOnJoint(theEnemy, 0, {0, 50, -40})`).
const MOUTH_JOINT: usize = 0;
const MOUTH_OFFSET: Vec3 = Vec3::new(0.0, 50.0, -40.0);

/// What a spider is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state here plays the animation of the
/// same name (`SPIDER_ANIM_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpiderState {
    /// Hidden high up, out of everyone's collision.
    #[default]
    Wait,
    /// Dropping on its thread.
    Drop,
    Walk,
    /// Spitting web (`SPIDER_ANIM_ATTACK`).
    Spit,
    Die,
    FallOnButt,
    GetOffButt,
    /// Pouncing on a webbed player.
    Jump,
}

impl SpiderState {
    /// The state's animation (`SPIDER_ANIM_*`).
    pub const fn anim(self) -> usize {
        match self {
            Self::Wait => 0,
            Self::Drop => 1,
            Self::Walk => 2,
            Self::Spit => 3,
            Self::Die => 4,
            Self::FallOnButt => 5,
            Self::GetOffButt => 6,
            Self::Jump => 7,
        }
    }
}

/// A spider's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct SpiderBrain {
    pub state: SpiderState,
    /// Seconds left on its butt (`ButtTimer`).
    pub butt_timer: f32,
    /// The thread it drops on (`ChainNode`), until it lands.
    pub thread: Option<Entity>,
}

impl SpiderBrain {
    /// Switches to `state` and starts its animation (`SetSkeletonAnim`).
    fn set_state(&mut self, animator: Option<&mut SkeletonAnimator>, state: SpiderState) {
        self.state = state;
        if let Some(animator) = animator {
            animator.set_anim(state.anim());
        }
    }

    /// Switches to `state`, blending into its animation at `rate` per
    /// second (`MorphToSkeletonAnim`).
    fn morph_state(
        &mut self,
        animator: Option<&mut SkeletonAnimator>,
        state: SpiderState,
        rate: f32,
    ) {
        self.state = state;
        if let Some(animator) = animator {
            animator.morph_to(state.anim(), rate);
        }
    }
}

/// The collision kinds of a spider that has dropped
/// (`CTYPE_ENEMY|CTYPE_KICKABLE|CTYPE_AUTOTARGET|CTYPE_SPIKED`).
fn dropped_kinds() -> LayerMask {
    LayerMask::from([
        CollisionKind::Enemy,
        CollisionKind::Kickable,
        CollisionKind::AutoTarget,
        CollisionKind::Spiked,
    ])
}

/// The web models of a level type: only the Forest and Night levels have
/// them (the `type[gLevelType]` tables of Enemy_Spider.c).
fn web_models(level_type: LevelType) -> Option<WebModels> {
    let first = match level_type {
        // `FOREST_MObjType_Thread`
        LevelType::Forest => 3,
        // `NIGHT_MObjType_Thread`
        LevelType::Night => 4,
        _ => return None,
    };
    let model = |offset: usize| ModelRef::new(ModelFile::Level1, first + offset);
    Some(WebModels {
        thread: model(0),
        bullet: model(1),
        sphere: model(2),
    })
}

/// Port of `AddEnemy_Spider` (original/src/Enemies/Enemy_Spider.c): a
/// spider hidden high above the item, out of everyone's collision
/// (`CType = 0`), with its thread.
fn add_spider(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !enemies.can_spawn(EnemyKind::Spider, MAX_SPIDERS) {
        return false;
    }
    let mut def = EnemySkeleton::new(
        EnemyKind::Spider,
        SkeletonType::Spider,
        spawn.position,
        SPIDER_SCALE,
    )
    .from_item(spawn.index)
    // A negative foot offset lifts it.
    .foot_offset(-SPIDER_START_YOFF)
    .collision_box(SPIDER_BOX)
    .solid(SolidSides::NOT_TOP)
    .health(SPIDER_HEALTH)
    .damage(SPIDER_DAMAGE)
    .shadow(SPIDER_SHADOW_SCALE);
    def.kinds = LayerMask::NONE;
    let Some(spider) = enemies.spawn(def) else {
        return false;
    };
    let commands = enemies.commands();
    commands
        .entity(spider)
        .insert((SpiderBrain::default(), Visibility::Hidden));
    thread::give_thread(commands, spider);
    true
}

/// Port of `PrimeEnemy_Spider` (original/src/Enemies/Enemy_Spider.c): a
/// spider walking its spline, on the floor, with no thread. Like every
/// prime routine it doesn't check the enemy counts.
fn prime_spider(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, SPIDER_SPLINE_SPEED);
    let Some(spider) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::Spider,
            SkeletonType::Spider,
            spawn.position,
            SPIDER_SCALE,
        )
        .on_spline(on_spline)
        .anim(SpiderState::Walk.anim())
        .foot_offset(-SPIDER_FOOT_OFFSET)
        .collision_box(SPIDER_BOX)
        .kickable()
        .solid(SolidSides::NOT_TOP)
        .health(SPIDER_HEALTH)
        .damage(SPIDER_DAMAGE)
        .shadow(SPIDER_SHADOW_SCALE),
    ) else {
        return false;
    };
    enemies.commands().entity(spider).insert(SpiderBrain {
        state: SpiderState::Walk,
        ..default()
    });
    true
}

/// Kills a spider: it leaves its spline, never comes back, becomes a plain
/// obstacle and dies. It never deletes the spider.
///
/// Port of `KillSpider` (original/src/Enemies/Enemy_Spider.c).
fn kill_spider(
    commands: &mut Commands,
    spider: Entity,
    brain: &mut SpiderBrain,
    animator: Option<&mut SkeletonAnimator>,
) {
    let mut entity = commands.entity(spider);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    entity.insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
    if brain.state != SpiderState::Die {
        brain.set_state(animator, SpiderState::Die);
    }
}

/// What [`knock_spider_on_butt`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knock {
    /// It was on its butt already, and nothing happened.
    Ignored,
    /// It went down; the player slows down.
    KnockedDown,
}

/// The parts of a spider that its knock changes.
struct KnockParts<'a> {
    entity: Entity,
    brain: &'a mut SpiderBrain,
    velocity: &'a mut Velocity,
    health: &'a mut Health,
    animator: Option<&'a mut SkeletonAnimator>,
}

/// Knocks a spider on its butt with `velocity` and hurts it by `damage`,
/// killing it if that was the last of its health.
///
/// Port of `KnockSpiderOnButt` (original/src/Enemies/Enemy_Spider.c), but
/// for slowing the player down, which the callers do with the player they
/// know.
fn knock_spider_on_butt(
    spider: KnockParts,
    commands: &mut Commands,
    velocity: Vec3,
    damage: f32,
) -> Knock {
    if spider.brain.state == SpiderState::FallOnButt {
        return Knock::Ignored;
    }
    detach_enemy_from_spline(&mut commands.entity(spider.entity));
    **spider.velocity = velocity;
    let mut animator = spider.animator;
    spider.brain.morph_state(
        animator.as_deref_mut(),
        SpiderState::FallOnButt,
        FALL_ON_BUTT_MORPH_RATE,
    );
    spider.brain.butt_timer = BUTT_TIME;
    if spider.health.lose(damage) {
        kill_spider(commands, spider.entity, spider.brain, animator);
    }
    Knock::KnockedDown
}

/// What the kick and ball handlers need of a spider.
type KnockQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SpiderBrain,
        &'static mut Velocity,
        &'static mut Health,
        &'static EnemyModel,
    ),
    Without<Player>,
>;

/// The players' velocities, which knocking a spider down slows.
type PlayerVelocities<'w, 's> =
    Query<'w, 's, &'static mut Velocity, (With<Player>, Without<SpiderBrain>)>;

/// The kick knocks a spider on its butt.
///
/// Port of the `SKELETON_TYPE_SPIDER` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_spiders(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut spiders: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<SpiderBrain>>,
    mut players: PlayerVelocities,
) {
    for kick in kicks.read() {
        let Ok((mut brain, mut velocity, mut health, model)) = spiders.get_mut(kick.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        let spider = KnockParts {
            entity: kick.enemy,
            brain: &mut brain,
            velocity: &mut velocity,
            health: &mut health,
            animator: animator.as_deref_mut(),
        };
        let knock = knock_spider_on_butt(
            spider,
            &mut commands,
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

/// A fast enough ball knocks a spider on its butt.
///
/// Port of `BallHitSpider` (original/src/Enemies/Enemy_Spider.c).
fn ball_hit_spiders(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut spiders: KnockQuery,
    mut animators: Query<&mut SkeletonAnimator, Without<SpiderBrain>>,
    mut players: PlayerVelocities,
) {
    for hit in hits.read() {
        if hit.ball_speed <= SPIDER_KNOCKDOWN_SPEED {
            continue;
        }
        let Ok((mut brain, mut velocity, mut health, model)) = spiders.get_mut(hit.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        let spider = KnockParts {
            entity: hit.enemy,
            brain: &mut brain,
            velocity: &mut velocity,
            health: &mut health,
            animator: animator.as_deref_mut(),
        };
        let knock = knock_spider_on_butt(
            spider,
            &mut commands,
            ball_knock(hit.ball_velocity),
            BALL_DAMAGE,
        );
        // `BallHitSpider` plays the sound even when the knock is ignored.
        // Sound: EFFECT_POUND at the spider, pitch kMiddleC+2, volume 2.0.
        if knock == Knock::KnockedDown
            && let Ok(mut player) = players.get_mut(hit.player)
        {
            **player *= PLAYER_SLOWDOWN;
        }
    }
}

/// The velocity a ball going at `ball_velocity` knocks a spider over with.
fn ball_knock(ball_velocity: Vec3) -> Vec3 {
    ball_velocity * BALL_KNOCK_SHARE + Vec3::Y * BALL_KNOCK_RISE
}

/// A spider whose health ran out dies.
///
/// Port of the `ENEMY_KIND_SPIDER` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_spiders(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut spiders: Query<(&mut SpiderBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator, Without<SpiderBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = spiders.get_mut(kill.enemy) else {
            continue;
        };
        let mut animator = animators.get_mut(model.0).ok();
        kill_spider(
            &mut commands,
            kill.enemy,
            &mut brain,
            animator.as_deref_mut(),
        );
    }
}

/// How fast a dropping spider `height` units above the ground falls, in
/// units per second (downward is negative). Port of the drop speed in
/// `MoveSpider_Drop`.
fn drop_velocity(height: f32) -> f32 {
    -(height * DROP_SPEED_PER_UNIT + DROP_MIN_SPEED)
}

/// What a walking spider does about the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkingAttack {
    None,
    Spit,
    Pounce,
}

/// Whether a walking spider, `aim` radians off a player `distance` units
/// away, spits web at it or, if it is webbed already, pounces on it. Port
/// of the attack checks in `MoveSpider_Walking`.
fn walking_attack(player_webbed: bool, aim: f32, distance: f32) -> WalkingAttack {
    if distance >= SPIDER_ATTACK_DIST {
        WalkingAttack::None
    } else if player_webbed {
        WalkingAttack::Pounce
    } else if aim < SPIT_AIM {
        WalkingAttack::Spit
    } else {
        WalkingAttack::None
    }
}

/// The velocity a spider at `from` pounces on a player at `to` with: it
/// covers the distance in x and z in one second, leaping up. Port of
/// `StartSpiderJump`.
fn pounce_velocity(from: Vec3, to: Vec3) -> Vec3 {
    Vec3::new(to.x - from.x, SPIDER_JUMPDY, to.z - from.z)
}

/// Turns toward `target` at [`SPIDER_TURN_SPEED`] for `dt` seconds and
/// returns the angle still to turn (`TurnObjectTowardTarget`).
fn turn_toward_target(transform: &mut Transform, target: Vec2, dt: f32) -> f32 {
    let (yaw, aim) = turn_toward(
        yaw_of(transform.rotation),
        transform.translation.xz(),
        target,
        SPIDER_TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
    aim
}

/// Falls under gravity, slowed by `friction` (per frame at 60 fps) if
/// `slow`, and moves (`MoveEnemy`).
fn tumble(body: &mut super::EnemyBodyItem, friction: f32, slow: bool, dt: f32) {
    let mut velocity = **body.velocity;
    if slow {
        apply_friction(&mut velocity, per_frame_friction(friction), dt);
    }
    velocity.y -= ENEMY_GRAVITY * dt;
    **body.velocity = velocity;
    move_enemy(&mut body.transform.translation, velocity, dt);
}

/// The player a spider goes for: the nearest one in x and z.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    at: Vec3,
    /// It is the bug caught in a web.
    webbed: bool,
}

/// The nearest player to `from`, by `CalcQuickDistance` (`gMyCoord`).
fn nearest_target(from: Vec3, players: &[Target]) -> Option<Target> {
    players.iter().copied().min_by(|a, b| {
        quick_distance(from.xz(), a.at.xz()).total_cmp(&quick_distance(from.xz(), b.at.xz()))
    })
}

/// The parts of a spider's model that its move reads.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SkeletonAnimator,
        &'static mut AnimationFlags,
        Option<&'static SkeletonRig>,
        &'static Transform,
    ),
    (Without<SpiderBrain>, Without<SpiderThread>),
>;

/// The threads, which a dropping spider pulls along.
type ThreadQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (With<SpiderThread>, Without<SpiderBrain>, Without<Player>),
>;

/// Moves the spiders that aren't on a spline, by their state.
///
/// Port of `MoveSpider`, `MoveSpider_Waiting`, `MoveSpider_Drop`,
/// `MoveSpider_Walking`, `MoveSpider_Spit`, `MoveSpider_Death`,
/// `MoveSpider_FallOnButt`, `MoveSpider_GetOffButt`, `MoveSpider_Jump` and
/// `StartSpiderJump` (original/src/Enemies/Enemy_Spider.c). Leaving the
/// item window (`TrackTerrainItem`) is `DespawnOutOfRange`. Each spider
/// goes for the nearest player.
///
/// Unlike the original, a spider that a hurt kills in the collision of its
/// walk or its pounce ends its move there. The original carries on with the
/// walk's attack checks or the pounce's landing, which can switch the dead
/// spider back to spitting or walking, leaving it alive in all but its
/// collision; this follows the approved difference for the flying bee
/// (docs/design/phase3-content.md §7).
#[allow(clippy::too_many_arguments)]
fn move_spiders(
    mut commands: Commands,
    mut collision: EnemyCollision,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    mut spiders: Query<
        (EnemyBody, &mut SpiderBrain, &EnemyModel, &mut Visibility),
        (Without<OnSpline>, Without<Player>, Without<SpiderThread>),
    >,
    mut animators: ModelQuery,
    mut threads: ThreadQuery,
    players: Query<
        (&Transform, &PlayerForm, Option<&BugState>),
        (With<Player>, Without<SpiderBrain>, Without<SpiderThread>),
    >,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    let targets: Vec<Target> = players
        .iter()
        .map(|(t, form, state)| Target {
            at: t.translation,
            webbed: is_webbed(*form, state.copied()),
        })
        .collect();
    let web = web_models(level.def().level_type);
    for (mut body, mut brain, model, mut visibility) in &mut spiders {
        let spider = body.entity;
        let Ok((mut animator, mut flags, rig, model_transform)) = animators.get_mut(model.0) else {
            continue;
        };
        let coord = body.transform.translation;
        let target = nearest_target(coord, &targets).unwrap_or(Target {
            at: coord,
            webbed: false,
        });
        let player = target.at;

        // Waiting is checked first, as `MoveSpider_Waiting` calls the drop
        // at once.
        if brain.state == SpiderState::Wait {
            if quick_distance(coord.xz(), player.xz()) >= SPIDER_DROP_DIST {
                continue;
            }
            *visibility = Visibility::Inherited;
            brain.set_state(Some(&mut animator), SpiderState::Drop);
            if let Some(Ok((_, mut thread_visibility))) = brain.thread.map(|t| threads.get_mut(t)) {
                *thread_visibility = Visibility::Inherited;
            }
            commands
                .entity(spider)
                .insert(CollisionLayers::new(dropped_kinds(), LayerMask::NONE));
        }

        match brain.state {
            SpiderState::Wait => {}
            SpiderState::Drop => {
                let mut at = body.transform.translation;
                let ground = map.floor_height(at.x, at.z);
                body.velocity.y = drop_velocity(at.y - ground);
                at.y += body.velocity.y * dt;
                if at.y - SPIDER_FOOT_OFFSET <= ground {
                    at.y = ground + SPIDER_FOOT_OFFSET;
                    brain.morph_state(Some(&mut animator), SpiderState::Walk, WALK_MORPH_RATE);
                    if let Some(thread) = brain.thread.take() {
                        thread::let_go_of_thread(&mut commands, thread);
                        place_thread(&mut threads, thread, at);
                    }
                }
                body.transform.translation = at;
                turn_toward_target(&mut body.transform, player.xz(), dt);
                if let Some(thread) = brain.thread {
                    place_thread(&mut threads, thread, at);
                }
            }
            SpiderState::Walk => {
                animator.speed = SPIDER_WALK_ANIM_SPEED;
                let aim = turn_toward_target(&mut body.transform, player.xz(), dt);
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * SPIDER_WALK_SPEED;
                let mut velocity = **body.velocity;
                velocity.x = forward.x;
                velocity.z = forward.y;
                velocity.y -= ENEMY_GRAVITY * dt;
                **body.velocity = velocity;
                move_enemy(&mut body.transform.translation, velocity, dt);
                if collide(
                    &mut collision,
                    &mut commands,
                    &mut body,
                    &mut brain,
                    &mut animator,
                ) {
                    continue;
                }
                let at = body.transform.translation;
                let distance = quick_distance(at.xz(), player.xz());
                match walking_attack(target.webbed, aim, distance) {
                    WalkingAttack::Spit => {
                        brain.morph_state(Some(&mut animator), SpiderState::Spit, SPIT_MORPH_RATE);
                        flags.0[SHOOT_WEB_FLAG] = false;
                    }
                    WalkingAttack::Pounce => {
                        brain.morph_state(Some(&mut animator), SpiderState::Jump, JUMP_MORPH_RATE);
                        **body.velocity = pounce_velocity(at, player);
                    }
                    WalkingAttack::None => {}
                }
            }
            SpiderState::Spit => {
                if flags.0[SHOOT_WEB_FLAG] {
                    flags.0[SHOOT_WEB_FLAG] = false;
                    if let Some(web) = web {
                        // The joints are where the last frame left them,
                        // and so is the spider (`FindCoordOnJoint` reads
                        // the last `UpdateObject`'s matrix).
                        let base = Affine3A::from_rotation_translation(
                            body.transform.rotation,
                            **body.previous,
                        ) * model_transform.compute_affine();
                        let mouth = rig
                            .and_then(|rig| joint_position(rig, MOUTH_JOINT, MOUTH_OFFSET, base))
                            .unwrap_or(body.transform.translation);
                        web::shoot_web(
                            &mut commands,
                            &mut models,
                            web.bullet,
                            mouth,
                            yaw_of(body.transform.rotation),
                        );
                    }
                }
                if animator.has_stopped {
                    brain.set_state(Some(&mut animator), SpiderState::Walk);
                }
                collide(
                    &mut collision,
                    &mut commands,
                    &mut body,
                    &mut brain,
                    &mut animator,
                );
            }
            SpiderState::FallOnButt => {
                let on_ground = body.ground.on_ground;
                tumble(&mut body, BUTT_FRICTION_PER_FRAME, on_ground, dt);
                brain.butt_timer -= dt;
                if brain.butt_timer <= 0.0 {
                    brain.set_state(Some(&mut animator), SpiderState::GetOffButt);
                }
                collide(
                    &mut collision,
                    &mut commands,
                    &mut body,
                    &mut brain,
                    &mut animator,
                );
            }
            SpiderState::GetOffButt => {
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, true, dt);
                if animator.has_stopped {
                    brain.set_state(Some(&mut animator), SpiderState::Walk);
                }
                collide(
                    &mut collision,
                    &mut commands,
                    &mut body,
                    &mut brain,
                    &mut animator,
                );
            }
            SpiderState::Jump => {
                tumble(&mut body, 0.0, false, dt);
                if collide(
                    &mut collision,
                    &mut commands,
                    &mut body,
                    &mut brain,
                    &mut animator,
                ) {
                    continue;
                }
                if body.ground.on_ground {
                    **body.velocity = Vec3::ZERO;
                    brain.morph_state(Some(&mut animator), SpiderState::Walk, WALK_MORPH_RATE);
                }
            }
            SpiderState::Die => {
                let at = body.transform.translation;
                if culling.is_culled(at, **body.radius)
                    && quick_distance(at.xz(), player.xz()) > DEAD_DELETE_DIST
                {
                    commands.entity(spider).despawn();
                    continue;
                }
                let on_ground = body.ground.on_ground;
                tumble(&mut body, GET_UP_FRICTION_PER_FRAME, on_ground, dt);
                collision.collide(&mut body, death_enemy_collision_mask(), &mut |_, _| false);
            }
        }
    }
}

/// Collides a live spider (`DoEnemyCollisionDetect` with
/// `DEFAULT_ENEMY_COLLISION_CTYPES`), killing it if a hurt takes the last
/// of its health. Returns whether it was killed.
fn collide(
    collision: &mut EnemyCollision,
    commands: &mut Commands,
    body: &mut super::EnemyBodyItem,
    brain: &mut SpiderBrain,
    animator: &mut SkeletonAnimator,
) -> bool {
    // `KillSpider` never deletes, so it can wait until the collision ends.
    let mut killed = false;
    collision.collide(body, default_enemy_collision_mask(), &mut |_, _| {
        killed = true;
        false
    });
    if killed {
        kill_spider(commands, body.entity, brain, Some(animator));
    }
    killed
}

/// Hangs a thread above a spider at `at` (`threadObj->Coord.y = gCoord.y +
/// THREAD_YOFF`). The thread keeps its x and z.
fn place_thread(threads: &mut ThreadQuery, thread: Entity, at: Vec3) {
    if let Ok((mut transform, _)) = threads.get_mut(thread) {
        transform.translation.y = at.y + THREAD_YOFF;
    }
}

/// Moves the spiders along their splines, and lets one off to walk at the
/// player once the player is close.
///
/// Port of `MoveSpiderOnSpline` (original/src/Enemies/Enemy_Spider.c). The
/// collision box and shadow follow the transform by themselves.
fn move_spiders_on_spline(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut spiders: Query<
        (
            Entity,
            &mut Transform,
            &mut OnSpline,
            &PreviousPosition,
            &EnemyModel,
        ),
        With<SpiderBrain>,
    >,
    mut animators: Query<&mut SkeletonAnimator, Without<SpiderBrain>>,
    players: Query<&Transform, (With<Player>, Without<SpiderBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (spider, mut transform, mut on_spline, previous, model) in &mut spiders {
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;
        if !on_spline.visible {
            continue;
        }
        if let Ok(mut animator) = animators.get_mut(model.0) {
            animator.speed = SPIDER_SPLINE_ANIM_SPEED;
        }
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), previous.xz(), position);
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y = map.floor_height(position.x, position.y) + SPIDER_FOOT_OFFSET;

        let at = transform.translation;
        let near = players
            .iter()
            .any(|p| quick_distance(at.xz(), p.translation.xz()) < SPIDER_ATTACK_DIST);
        if near {
            // Off the spline it walks, with the walk animation it has had
            // since `PrimeEnemy_Spider`.
            detach_enemy_from_spline(&mut commands.entity(spider));
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn the_drop_slows_as_the_ground_nears() {
        assert_eq!(drop_velocity(0.0), -DROP_MIN_SPEED);
        assert_eq!(drop_velocity(1000.0), -1800.0);
        assert!(drop_velocity(100.0) > drop_velocity(500.0));
    }

    #[test]
    fn a_walking_spider_spits_at_a_close_player_it_faces_and_pounces_on_a_webbed_one() {
        assert_eq!(walking_attack(false, 0.5, 400.0), WalkingAttack::Spit);
        assert_eq!(walking_attack(false, 1.2, 400.0), WalkingAttack::None);
        assert_eq!(walking_attack(false, 0.5, 600.0), WalkingAttack::None);
        // Webbed already: it pounces however it is turned.
        assert_eq!(walking_attack(true, 2.0, 400.0), WalkingAttack::Pounce);
        assert_eq!(walking_attack(true, 0.5, 600.0), WalkingAttack::None);
    }

    #[test]
    fn a_pounce_reaches_the_player_in_a_second() {
        let v = pounce_velocity(Vec3::new(100.0, 0.0, 50.0), Vec3::new(400.0, 30.0, -50.0));
        assert_eq!(v, Vec3::new(300.0, SPIDER_JUMPDY, -100.0));
    }

    #[test]
    fn the_ball_knocks_a_spider_along_and_up() {
        let v = ball_knock(Vec3::new(1000.0, 0.0, -500.0));
        assert!(v.abs_diff_eq(Vec3::new(800.0, 250.0, -400.0), 1e-3));
    }

    #[test]
    fn only_the_forest_and_night_have_web_models() {
        let forest = web_models(LevelType::Forest).map(|m| m.sphere);
        assert_eq!(forest, Some(ModelRef::new(ModelFile::Level1, 5)));
        let night = web_models(LevelType::Night).map(|m| m.thread);
        assert_eq!(night, Some(ModelRef::new(ModelFile::Level1, 4)));
        assert_eq!(web_models(LevelType::Lawn), None);
    }

    #[test]
    fn the_spider_goes_for_the_nearest_player() {
        let near = Target {
            at: Vec3::new(10.0, 0.0, 0.0),
            webbed: true,
        };
        let far = Target {
            at: Vec3::new(500.0, 0.0, 0.0),
            webbed: false,
        };
        assert_eq!(nearest_target(Vec3::ZERO, &[far, near]), Some(near));
        assert_eq!(nearest_target(Vec3::ZERO, &[]), None);
    }

    fn knock(world: &mut World, spider: Entity, damage: f32) -> Knock {
        world
            .run_system_once(move |mut commands: Commands, mut spiders: KnockQuery| {
                let Ok((mut brain, mut velocity, mut health, _)) = spiders.get_mut(spider) else {
                    return Knock::Ignored;
                };
                let parts = KnockParts {
                    entity: spider,
                    brain: &mut brain,
                    velocity: &mut velocity,
                    health: &mut health,
                    animator: None,
                };
                knock_spider_on_butt(parts, &mut commands, Vec3::new(1.0, 2.0, 3.0), damage)
            })
            .expect("the system runs")
    }

    #[test]
    fn a_knock_sits_the_spider_down_once_and_can_kill_it() {
        let mut world = World::new();
        let model = world.spawn_empty().id();
        let spider = world
            .spawn((
                SpiderBrain {
                    state: SpiderState::Walk,
                    ..default()
                },
                Velocity::default(),
                Health(SPIDER_HEALTH),
                EnemyModel(model),
            ))
            .id();
        assert_eq!(knock(&mut world, spider, 0.1), Knock::KnockedDown);
        let brain = world.get::<SpiderBrain>(spider).copied();
        assert_eq!(brain.map(|b| b.state), Some(SpiderState::FallOnButt));
        assert_eq!(brain.map(|b| b.butt_timer), Some(BUTT_TIME));
        assert_eq!(
            world.get::<Velocity>(spider),
            Some(&Velocity(Vec3::new(1.0, 2.0, 3.0)))
        );
        // On its butt already: ignored.
        assert_eq!(knock(&mut world, spider, 1.0), Knock::Ignored);
        assert!(world.get::<CollisionLayers>(spider).is_none());

        world.entity_mut(spider).insert(SpiderBrain {
            state: SpiderState::Walk,
            ..default()
        });
        assert_eq!(knock(&mut world, spider, 1.0), Knock::KnockedDown);
        let brain = world.get::<SpiderBrain>(spider).copied();
        assert_eq!(brain.map(|b| b.state), Some(SpiderState::Die));
        let layers = world.get::<CollisionLayers>(spider).map(|l| l.memberships);
        assert_eq!(layers, Some(LayerMask::from(CollisionKind::Misc)));
    }
}
