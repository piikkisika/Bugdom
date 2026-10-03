//! The mosquito: it hovers high above the ground until the player comes
//! near, chases it and dives to bite. A bite that lands in the bug's head
//! holds the bug and sucks its blood; one that misses sticks in the ground
//! for a moment.
//!
//! Port of original/src/Enemies/Enemy_Mosquito.c. It is both a map item
//! and a spline item ([`kind::MOSQUITO`]), on the Pond level. A mosquito on
//! a spline leaves it to chase the player once the player is in range, and
//! then never flies home.
//!
//! The original dispatches on the mosquito's animation
//! (`myMoveTable[AnimNum]`); here that is [`MosquitoState`]. The two
//! flying animations (`MOSQUITO_ANIM_FLY` and `MOSQUITO_ANIM_FLYFULL`, the
//! one with a full belly) share [`MosquitoState::Flying`], and the move
//! reads which of them plays from the animator, as the original does.
//!
//! The bite holds the bug with [`HoldPlayer`] and lets go with
//! [`ReleasePlayer`]. The original then morphs the bug to standing
//! whatever it is doing at that moment, and so does `KillMosquito` when
//! it kills a mosquito that is sucking; [`ReleasePlayer`] only stands up a
//! bug that is still held, so the mosquito sets the bug's state itself
//! ([`Victims::stand_up`]).

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled,
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, HomePosition, KICK_SPEED,
    apply_friction, death_enemy_collision_mask, default_enemy_collision_mask,
    detach_enemy_from_spline, move_enemy, nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::combat::Health;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of};
use crate::objects::Shadows;
use crate::physics::PreviousPosition;
use crate::player::{
    BugState, Dying, Hold, HoldPlayer, HurtOutcome, HurtPlayer, InvincibleTimer, Player,
    PlayerForm, PlayerModel, ReleasePlayer, ShieldTimer, take_hurt,
};
use crate::skeleton::{SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::terrain::TerrainMap;

/// The most mosquitoes spawned from map items at once (`MAX_MOSQUITO`).
const MAX_MOSQUITO: usize = 4;

/// How close the player must come for a mosquito to chase it, in units
/// (`MOSQUITO_CHASE_RANGE`).
const CHASE_RANGE: f32 = 400.0;
/// How close a chasing mosquito must be to dive and bite, in units
/// (`MOSQUITO_BITE_RANGE`).
const BITE_RANGE: f32 = 160.0;
/// Turn speed, in radians per second (`MOSQUITO_TURN_SPEED`).
const TURN_SPEED: f32 = 4.5;
/// Flying speed when chasing, going home or diving, in units per second
/// (`MOSQUITO_CHASE_SPEED`).
const CHASE_SPEED: f32 = 460.0;
/// Speed along a spline, in baked points per second
/// (`MOSQUITO_SPLINE_SPEED`).
const SPLINE_SPEED: f32 = 200.0;
/// How far from home a mosquito from a map item chases before it flies
/// back, in units (`MAX_MOSQUITO_RANGE`).
const MAX_RANGE: f32 = CHASE_RANGE + 800.0;
/// How close to home a mosquito going home must get to wait again, in
/// units.
const HOME_RANGE: f32 = 150.0;
/// How fast a mosquito climbs back to its flying height after a bite, in
/// units per second (`MOSQUITO_MODE_REALIGN`).
const REALIGN_RISE_SPEED: f32 = 150.0;
/// How fast a diving mosquito speeds up downward, in units per second
/// squared (`MoveMosquito_Biting`).
const DIVE_ACCEL: f32 = 2000.0;
/// Seconds a stinger that hit the ground stays stuck (`StuckTimer`).
const STUCK_TIME: f32 = 1.1;

/// `MOSQUITO_HEALTH`.
const HEALTH: f32 = 1.0;
/// What touching a mosquito does to the player (`MOSQUITO_DAMAGE`).
const DAMAGE: f32 = 0.04;
/// What the bite does to the player (`PlayerGotHurt(nil, .2, ...)`).
const BITE_DAMAGE: f32 = 0.2;
/// Seconds of invincibility the bite gives the player.
const BITE_INVINCIBILITY: f32 = 2.0;
/// `MOSQUITO_SCALE`.
const SCALE: f32 = 0.8;
/// Height above the floor it flies at, in units
/// (`MOSQUITO_FLIGHT_HEIGHT`).
const FLIGHT_HEIGHT: f32 = 350.0;
/// How fast the wobble's phase turns, in radians per second
/// (`MosquitoWobbleOff`).
const WOBBLE_RATE: f32 = 11.0;
/// How far the mosquito wobbles up and down, in units.
const WOBBLE_HEIGHT: f32 = 10.0;
/// The Pond's water surface (`WATER_Y`).
const WATER_Y: f32 = 0.0;
/// How far above the water a mosquito on the Pond stays, in units.
const WATER_CLEARANCE: f32 = 150.0;
/// Shadow size (`AttachShadowToObject(newObj, 7, 7, false)`).
const SHADOW_SCALE: f32 = 7.0;
/// The collision box (`SetObjectCollisionBounds(newObj, 70,-40,-40,40,40,-40)`).
const COLLISION_BOX: CollisionBox = CollisionBox::new(70.0, -40.0, -40.0, 40.0, 40.0, -40.0);

/// The joint that carries the stinger (`MOSQUITO_HEAD_JOINT`), and the
/// stinger's tip in its space (`gTipOffset`).
const HEAD_JOINT: usize = 4;
const TIP_OFFSET: Vec3 = Vec3::new(0.0, -28.0, 92.0);
/// The bug's head joint (`BUG_LIMB_NUM_HEAD`), and where the stinger goes
/// in its space (`headOffset` in `MoveMosquito_Sucking`).
const PLAYER_HEAD_JOINT: usize = 7;
const PLAYER_HEAD_OFFSET: Vec3 = Vec3::new(0.0, 18.0, -17.0);

/// The animations (`MOSQUITO_ANIM_*`).
mod anim {
    pub const FLY: usize = 0;
    pub const BITE: usize = 1;
    pub const DEATH: usize = 2;
    pub const SUCK: usize = 3;
    pub const FLY_FULL: usize = 4;
}
/// Flying animations play at twice their speed (`AnimSpeed = 2.0`).
const FLY_ANIM_SPEED: f32 = 2.0;
/// How much of each blend happens per second: into the full flight when a
/// chase starts, into the bite, back to flying from a stuck stinger or the
/// water, into sucking, back to flying when the bite killed, and into the
/// full flight when the sucking is done.
const CHASE_MORPH_RATE: f32 = 6.0;
const BITE_MORPH_RATE: f32 = 3.0;
const UNSTUCK_MORPH_RATE: f32 = 3.0;
const WATER_MORPH_RATE: f32 = 5.0;
const SUCK_MORPH_RATE: f32 = 6.0;
const KILLED_PLAYER_MORPH_RATE: f32 = 6.0;
const FULL_MORPH_RATE: f32 = 5.0;

/// How fast the ball must go to knock a mosquito down, in units per
/// second (`MOSQUITO_KNOCKDOWN_SPEED`).
const KNOCKDOWN_SPEED: f32 = 1400.0;
/// How much of the ball's horizontal velocity a knocked-down mosquito
/// takes.
const BALL_KNOCK_SHARE: f32 = 0.5;
/// How fast the ball sends a mosquito up, in units per second.
const BALL_KNOCK_RISE: f32 = 400.0;
/// Friction on a dead mosquito on the ground, per frame at 60 fps
/// (`ApplyFrictionToDeltas(60.0, ...)`).
const DEATH_FRICTION_PER_FRAME: f32 = 60.0;
/// A dead mosquito out of view goes once it is this far from the player,
/// in units (`MoveMosquito_Death`).
const DEATH_DESPAWN_DISTANCE: f32 = 600.0;

pub struct MosquitoPlugin;

impl Plugin for MosquitoPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::MOSQUITO, add_mosquito)
            .register_spline_item_kind(kind::MOSQUITO, prime_mosquito)
            .add_systems(
                FixedUpdate,
                (
                    kick_mosquitoes.in_set(EnemySystems::Kicked),
                    (ball_hit_mosquitoes, move_mosquitoes)
                        .chain()
                        .in_set(EnemySystems::Move),
                    move_mosquitoes_on_spline.in_set(SplineSystems::Move),
                    kill_hurt_mosquitoes.in_set(EnemySystems::Killed),
                ),
            );
    }
}

/// What a mosquito is doing; it follows its animation, as the original
/// indexes its move table with `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MosquitoState {
    /// Flying, empty or full (`MoveMosquito_Flying`).
    #[default]
    Flying,
    /// Diving at the player, or stuck in the ground
    /// (`MoveMosquito_Biting`).
    Biting,
    /// Sucking a bug's blood (`MoveMosquito_Sucking`).
    Sucking,
    /// Killed, falling (`MoveMosquito_Death`).
    Dying,
}

/// Where a flying mosquito is going (`MOSQUITO_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MosquitoMode {
    /// Hovering, turned toward the player, until it comes in range.
    #[default]
    Waiting,
    GoHome,
    Chase,
    /// Climbing back to its flying height after a bite.
    Realign,
}

/// A mosquito's own state.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct MosquitoBrain {
    pub state: MosquitoState,
    pub mode: MosquitoMode,
    /// The wobble's phase, in radians (`Wobble`).
    pub wobble: f32,
    /// Whether it flies home when too far from it (`HonorRange`).
    /// Mosquitoes that left a spline don't.
    pub honor_range: bool,
    /// Seconds left with the stinger stuck in the ground, while it is
    /// (`StuckInGround`, `StuckTimer`).
    pub stuck: Option<f32>,
    /// The player it bit, while it sucks (`gPlayerObj`).
    pub victim: Option<Entity>,
}

impl Default for MosquitoBrain {
    fn default() -> Self {
        Self {
            state: MosquitoState::Flying,
            mode: MosquitoMode::Waiting,
            wobble: 0.0,
            honor_range: true,
            stuck: None,
            victim: None,
        }
    }
}

impl MosquitoBrain {
    /// Advances the wobble by `dt` seconds and returns the height it adds.
    /// Port of `MosquitoWobbleOff`.
    fn wobble(&mut self, dt: f32) -> f32 {
        self.wobble += WOBBLE_RATE * dt;
        self.wobble.sin() * WOBBLE_HEIGHT
    }
}

/// The mosquito's skeleton, as `AddEnemy_Mosquito` and
/// `PrimeEnemy_Mosquito` set it up.
fn mosquito_skeleton(position: Vec2) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::Mosquito, SkeletonType::Mosquito, position, SCALE)
        .anim(anim::FLY)
        .foot_offset(-FLIGHT_HEIGHT)
        .health(HEALTH)
        .damage(DAMAGE)
        .kickable()
        .solid(SolidSides::NOT_TOP)
        .collision_box(COLLISION_BOX)
        .shadow(SHADOW_SCALE)
}

/// Port of `AddEnemy_Mosquito` (original/src/Enemies/Enemy_Mosquito.c).
fn add_mosquito(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !enemies.can_spawn(EnemyKind::Mosquito, MAX_MOSQUITO) {
        return false;
    }
    let Some(mosquito) = enemies.spawn(mosquito_skeleton(spawn.position).from_item(spawn.index))
    else {
        return false;
    };
    enemies
        .commands()
        .entity(mosquito)
        .insert(MosquitoBrain::default());
    true
}

/// Port of `PrimeEnemy_Mosquito` (original/src/Enemies/Enemy_Mosquito.c),
/// which, like every prime routine, doesn't check the enemy counts.
fn prime_mosquito(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, SPLINE_SPEED);
    let Some(mosquito) = enemies.spawn(mosquito_skeleton(spawn.position).on_spline(on_spline))
    else {
        return false;
    };
    enemies
        .commands()
        .entity(mosquito)
        .insert(MosquitoBrain::default());
    true
}

/// What one step of flying did.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FlightStep {
    yaw: f32,
    coord: Vec3,
    /// The horizontal velocity, if the step set it (`gDelta.x`,
    /// `gDelta.z`); waiting and climbing leave it.
    velocity: Option<Vec2>,
    /// The player came in range of a waiting mosquito.
    start_chase: bool,
    /// Close enough to the player to dive and bite.
    bite: bool,
}

/// Turns toward `target` and flies forward at [`CHASE_SPEED`] for `dt`
/// seconds. Returns the new yaw, the horizontal velocity and the new
/// position in x and z.
fn fly_toward(yaw: f32, position: Vec2, target: Vec2, dt: f32) -> (f32, Vec2, Vec2) {
    let (yaw, _) = turn_toward(yaw, position, target, TURN_SPEED * dt);
    let velocity = yaw_forward(yaw) * CHASE_SPEED;
    (yaw, velocity, position + velocity * dt)
}

/// One step of a flying mosquito, changing its mode as it goes. `floor`
/// gives the floor's height at a point in x and z.
///
/// Port of the mode switch in `MoveMosquito_Flying`
/// (original/src/Enemies/Enemy_Mosquito.c).
fn fly_step(
    brain: &mut MosquitoBrain,
    yaw: f32,
    coord: Vec3,
    home: Vec2,
    player: Vec2,
    floor: impl Fn(Vec2) -> f32,
    dt: f32,
) -> FlightStep {
    let mut step = FlightStep {
        yaw,
        coord,
        velocity: None,
        start_chase: false,
        bite: false,
    };
    match brain.mode {
        MosquitoMode::Realign => {
            if coord.y < floor(coord.xz()) + FLIGHT_HEIGHT + brain.wobble(dt) {
                step.coord.y += REALIGN_RISE_SPEED * dt;
            } else {
                brain.mode = MosquitoMode::GoHome;
            }
        }
        MosquitoMode::Waiting => {
            (step.yaw, _) = turn_toward(yaw, coord.xz(), player, TURN_SPEED * dt);
            if quick_distance(coord.xz(), player) < CHASE_RANGE {
                brain.mode = MosquitoMode::Chase;
                step.start_chase = true;
            }
            step.coord.y = floor(coord.xz()) + FLIGHT_HEIGHT + brain.wobble(dt);
        }
        MosquitoMode::GoHome => {
            let (yaw, velocity, xz) = fly_toward(yaw, coord.xz(), home, dt);
            step.yaw = yaw;
            step.velocity = Some(velocity);
            step.coord = Vec3::new(xz.x, floor(xz) + FLIGHT_HEIGHT + brain.wobble(dt), xz.y);
            if quick_distance(xz, home) < HOME_RANGE {
                brain.mode = MosquitoMode::Waiting;
            }
        }
        MosquitoMode::Chase => {
            let (yaw, velocity, xz) = fly_toward(yaw, coord.xz(), player, dt);
            step.yaw = yaw;
            step.velocity = Some(velocity);
            step.coord = Vec3::new(xz.x, floor(xz) + FLIGHT_HEIGHT + brain.wobble(dt), xz.y);
            step.bite = quick_distance(xz, player) < BITE_RANGE;
            if brain.honor_range && quick_distance(home, xz) > MAX_RANGE {
                brain.mode = MosquitoMode::GoHome;
            }
        }
    }
    step
}

/// One step of a diving mosquito: it turns toward the player, flies at it
/// and drops ever faster. Returns the new yaw, position and velocity.
///
/// Port of the move in `MoveMosquito_Biting`
/// (original/src/Enemies/Enemy_Mosquito.c).
fn dive_step(yaw: f32, coord: Vec3, velocity: Vec3, player: Vec2, dt: f32) -> (f32, Vec3, Vec3) {
    let (yaw, _) = turn_toward(yaw, coord.xz(), player, TURN_SPEED * dt);
    let forward = yaw_forward(yaw) * CHASE_SPEED;
    let velocity = Vec3::new(forward.x, velocity.y - DIVE_ACCEL * dt, forward.y);
    (yaw, coord + velocity * dt, velocity)
}

/// The bite's hurt (`PlayerGotHurt(nil, .2, true, false, false, 2.0)`):
/// through the shield, without a knock.
fn bite_hurt(player: Entity) -> HurtPlayer {
    HurtPlayer {
        player,
        source: None,
        damage: BITE_DAMAGE,
        knock: false,
        invincible_for: BITE_INVINCIBILITY,
        override_shield: true,
        torch_if_killed: false,
    }
}

/// Whether `hurt` will kill the player, as `gPlayerGotKilledFlag` tells
/// the original right after `PlayerGotHurt`. The hurt itself is applied
/// later in the tick ([`PlayerSystems::Hurt`](crate::player::PlayerSystems::Hurt)),
/// so this works it out on copies.
fn hurt_kills(
    hurt: &HurtPlayer,
    dying: bool,
    shield: ShieldTimer,
    mut health: Health,
    mut invincible: InvincibleTimer,
) -> bool {
    take_hurt(hurt, dying, shield, &mut health, &mut invincible) == HurtOutcome::Killed
}

/// Where the stinger goes so that its tip is at `head`, given where the
/// tip is with the mosquito at `coord` (`MoveMosquito_Sucking`).
fn stinger_in_head(coord: Vec3, tip: Vec3, head: Vec3) -> Vec3 {
    head - (tip - coord)
}

/// Kills a mosquito: it leaves its spline, stops counting as an enemy for
/// collisions, loses its shadow and falls dead. Returns the player it was
/// sucking, for the caller to stand up. The caller sets its velocity.
///
/// Port of `KillMosquito` (original/src/Enemies/Enemy_Mosquito.c), but for
/// the delta and the player.
fn kill_mosquito(
    commands: &mut Commands,
    mosquito: Entity,
    brain: &mut MosquitoBrain,
    animator: Option<&mut SkeletonAnimator>,
    shape: CollisionBox,
) -> Option<Entity> {
    let victim = if brain.state == MosquitoState::Sucking {
        brain.victim.take()
    } else {
        None
    };
    // Sound: stop EFFECT_BUZZ.
    let mut entity = commands.entity(mosquito);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    // `CType = CTYPE_MISC; BottomOff = 0;`, keeping its solid sides.
    let shape = CollisionBox {
        bottom: 0.0,
        ..shape
    };
    entity
        .insert(solid_object(
            vec![shape],
            CollisionKind::Misc,
            SolidSides::NOT_TOP,
        ))
        .despawn_related::<Shadows>();
    brain.state = MosquitoState::Dying;
    if let Some(animator) = animator
        && animator.anim != anim::DEATH
    {
        animator.set_anim(anim::DEATH);
    }
    victim
}

/// The players' states, for the mosquito to stand a bug up.
#[derive(bevy::ecs::system::SystemParam)]
struct Victims<'w, 's> {
    players: Query<
        'w,
        's,
        (&'static PlayerForm, &'static mut BugState, Has<Dying>),
        (With<Player>, Without<MosquitoBrain>),
    >,
}

impl Victims<'_, '_> {
    /// Stands the bug up, whatever it is doing
    /// (`MorphToSkeletonAnim(gPlayerObj->Skeleton, PLAYER_ANIM_STAND, 7)`
    /// in `KillMosquito` and `MoveMosquito_Sucking`). The blend into
    /// standing is the bug's own: 7 per second from the blood suck, 6
    /// from other states.
    ///
    /// Intentional difference (needs approval): a dying bug is left
    /// alone. The original stands even a dead bug up, which ends its
    /// death animation and gives the player control until the level
    /// restarts it.
    fn stand_up(&mut self, player: Entity) {
        if let Ok((form, mut state, dying)) = self.players.get_mut(player)
            && *form == PlayerForm::Bug
            && !dying
        {
            *state = BugState::Stand;
        }
    }
}

/// The mosquitoes and their models, for the message handlers that kill
/// them.
#[derive(bevy::ecs::system::SystemParam)]
struct MosquitoKiller<'w, 's> {
    commands: Commands<'w, 's>,
    mosquitoes: Query<
        'w,
        's,
        (
            &'static mut MosquitoBrain,
            &'static mut crate::physics::Velocity,
            &'static EnemyModel,
            &'static CollisionBoxes,
        ),
    >,
    animators: Query<'w, 's, &'static mut SkeletonAnimator, Without<MosquitoBrain>>,
    victims: Victims<'w, 's>,
}

impl MosquitoKiller<'_, '_> {
    /// [`kill_mosquito`] for `entity`, if it is a mosquito, sending it off
    /// with `velocity` and standing up the bug it was sucking. Returns
    /// whether it was a mosquito.
    fn kill(&mut self, entity: Entity, velocity: Vec3) -> bool {
        let Ok((mut brain, mut v, model, boxes)) = self.mosquitoes.get_mut(entity) else {
            return false;
        };
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        let mut animator = self.animators.get_mut(model.0).ok();
        let victim = kill_mosquito(
            &mut self.commands,
            entity,
            &mut brain,
            animator.as_deref_mut(),
            shape,
        );
        // `KillMosquito` sets the delta even for a mosquito that is dead
        // already.
        **v = velocity;
        if let Some(victim) = victim {
            self.victims.stand_up(victim);
        }
        true
    }
}

/// The kick kills a mosquito, sending it flying.
///
/// Port of the `SKELETON_TYPE_MOSQUITO` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_mosquitoes(mut kicks: MessageReader<EnemyKicked>, mut mosquitoes: MosquitoKiller) {
    for kick in kicks.read() {
        mosquitoes.kill(kick.enemy, kick.knock(KICK_SPEED, KICK_SPEED));
    }
}

/// A fast enough ball knocks a mosquito dead.
///
/// Port of `BallHitMosquito` (original/src/Enemies/Enemy_Mosquito.c).
fn ball_hit_mosquitoes(mut hits: MessageReader<BallHitEnemy>, mut mosquitoes: MosquitoKiller) {
    for hit in hits.read() {
        if hit.ball_speed <= KNOCKDOWN_SPEED {
            continue;
        }
        let push = hit.ball_velocity * BALL_KNOCK_SHARE;
        if mosquitoes.kill(hit.enemy, Vec3::new(push.x, BALL_KNOCK_RISE, push.z)) {
            // Sound: EFFECT_POUND at the mosquito, pitch kMiddleC+2, volume 2.0.
        }
    }
}

/// A mosquito whose health ran out dies where it is.
///
/// Port of the `ENEMY_KIND_MOSQUITO` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c), which calls `KillMosquito` with no
/// delta.
fn kill_hurt_mosquitoes(mut killed: MessageReader<EnemyKilled>, mut mosquitoes: MosquitoKiller) {
    for kill in killed.read() {
        mosquitoes.kill(kill.enemy, kill.knock);
    }
}

/// The models' transforms and rigs, for the joint points.
type RigQuery<'w, 's> =
    Query<'w, 's, (&'static Transform, Option<&'static SkeletonRig>), Without<MosquitoBrain>>;

/// Where a point in a joint's space of an object's model is in the world
/// (`FindCoordOnJoint`), with the object's root at `root`. The joints are
/// where the last animation step left them.
fn model_point(
    rigs: &RigQuery,
    model: Entity,
    root: &Transform,
    joint: usize,
    offset: Vec3,
) -> Option<Vec3> {
    let (model_transform, rig) = rigs.get(model).ok()?;
    let base = Affine3A::from_rotation_translation(root.rotation, root.translation)
        * model_transform.compute_affine();
    joint_position(rig?, joint, offset, base)
}

/// The players, as the mosquitoes see them.
type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static CollisionBoxes,
        &'static avian3d::prelude::CollisionLayers,
        &'static PlayerModel,
        &'static Health,
        &'static InvincibleTimer,
        &'static ShieldTimer,
        Has<Dying>,
    ),
    (With<Player>, Without<MosquitoBrain>),
>;

/// Moves the mosquitoes that aren't on a spline: flying, biting, sucking
/// or dying.
///
/// Port of `MoveMosquito`, `MoveMosquito_Flying`, `MoveMosquito_Biting`,
/// `MoveMosquito_Sucking`, `MoveMosquito_Death` and `UpdateMosquito`
/// (original/src/Enemies/Enemy_Mosquito.c). Going out of range is the
/// items' `DespawnOutOfRange`. Each mosquito goes for the nearest player,
/// and bites whichever player its stinger finds.
#[allow(clippy::too_many_arguments)]
fn move_mosquitoes(
    mut commands: Commands,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    mut collision: EnemyCollision,
    mut mosquitoes: Query<
        (EnemyBody, &mut MosquitoBrain, &EnemyModel, &HomePosition),
        Without<OnSpline>,
    >,
    mut animators: Query<&mut SkeletonAnimator, Without<MosquitoBrain>>,
    rigs: RigQuery,
    players: PlayerQuery,
    mut victims: Victims,
    culling: EnemyCulling,
    mut holds: MessageWriter<HoldPlayer>,
    mut releases: MessageWriter<ReleasePlayer>,
    mut hurts: MessageWriter<HurtPlayer>,
) {
    let dt = collision.dt();
    if dt <= 0.0 {
        return;
    }
    let on_pond = level.def().level_type == LevelType::Pond;
    let floor = |xz: Vec2| map.floor_height(xz.x, xz.y);
    for (mut body, mut brain, model, home) in &mut mosquitoes {
        let mosquito = body.entity;
        let shape = body.boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        let Ok(mut animator) = animators.get_mut(model.0) else {
            continue;
        };
        let start = body.transform.translation;
        let target = nearest_player(start, players.iter().map(|p| p.1.translation));

        if brain.state == MosquitoState::Dying {
            // Gone once the camera stopped seeing it, away from the player.
            let far =
                target.is_none_or(|p| quick_distance(start.xz(), p.xz()) > DEATH_DESPAWN_DISTANCE);
            if far && culling.is_culled(start, **body.radius) {
                commands.entity(mosquito).despawn();
                continue;
            }
            let mut velocity = **body.velocity;
            if body.ground.on_ground {
                apply_friction(
                    &mut velocity,
                    per_frame_friction(DEATH_FRICTION_PER_FRAME),
                    dt,
                );
            }
            velocity.y -= ENEMY_GRAVITY * dt;
            **body.velocity = velocity;
            move_enemy(&mut body.transform.translation, velocity, dt);
            collision.collide(&mut body, death_enemy_collision_mask(), &mut |_, _| false);
            continue;
        }

        let Some(target) = target else {
            continue;
        };
        let yaw = yaw_of(body.transform.rotation);

        match brain.state {
            MosquitoState::Flying => {
                animator.speed = FLY_ANIM_SPEED;
                if animator.anim == anim::FLY_FULL && culling.is_culled(start, **body.radius) {
                    animator.set_anim(anim::FLY);
                }
                let step = fly_step(&mut brain, yaw, start, home.xz(), target.xz(), floor, dt);
                if step.start_chase && animator.anim != anim::FLY {
                    animator.morph_to(anim::FLY_FULL, CHASE_MORPH_RATE);
                }
                if step.bite {
                    animator.morph_to(anim::BITE, BITE_MORPH_RATE);
                    brain.state = MosquitoState::Biting;
                    brain.stuck = None;
                }
                let mut coord = step.coord;
                if on_pond {
                    coord.y = coord.y.max(WATER_Y + WATER_CLEARANCE);
                }
                if let Some(v) = step.velocity {
                    body.velocity.x = v.x;
                    body.velocity.z = v.y;
                }
                body.transform.translation = coord;
                body.transform.rotation = Quat::from_rotation_y(step.yaw);
            }
            MosquitoState::Biting => {
                if let Some(timer) = brain.stuck {
                    let timer = timer - dt;
                    brain.stuck = Some(timer);
                    if timer <= 0.0 {
                        animator.morph_to(anim::FLY, UNSTUCK_MORPH_RATE);
                        brain.stuck = None;
                        brain.mode = MosquitoMode::Realign;
                        brain.state = MosquitoState::Flying;
                    }
                } else {
                    let (yaw, mut coord, mut velocity) =
                        dive_step(yaw, start, **body.velocity, target.xz(), dt);
                    if on_pond && coord.y < WATER_Y + WATER_CLEARANCE {
                        coord.y = WATER_Y + WATER_CLEARANCE;
                        animator.morph_to(anim::FLY, WATER_MORPH_RATE);
                        brain.mode = MosquitoMode::Realign;
                        brain.state = MosquitoState::Flying;
                    }
                    body.transform.translation = coord;
                    body.transform.rotation = Quat::from_rotation_y(yaw);

                    let tip = model_point(&rigs, model.0, &body.transform, HEAD_JOINT, TIP_OFFSET);
                    if let Some(tip) = tip {
                        let ground = floor(tip.xz());
                        if tip.y <= ground {
                            body.transform.translation.y += ground - tip.y;
                            velocity = Vec3::ZERO;
                            brain.stuck = Some(STUCK_TIME);
                        }
                    }
                    **body.velocity = velocity;

                    // `DoSimplePointCollision(&tipPt, CTYPE_PLAYER)`: a held
                    // bug has no `CType`, so no other mosquito bites it.
                    let bitten = tip.filter(|_| brain.stuck.is_none()).and_then(|tip| {
                        players.iter().find(|(_, transform, boxes, layers, ..)| {
                            layers.memberships.has_all(CollisionKind::Player)
                                && boxes
                                    .0
                                    .iter()
                                    .any(|b| b.at(transform.translation).contains(tip))
                        })
                    });
                    if let Some((player, _, _, _, _, health, invincible, shield, dying)) = bitten {
                        // Sound: EFFECT_SLURP at the mosquito.
                        animator.morph_to(anim::SUCK, SUCK_MORPH_RATE);
                        holds.write(HoldPlayer {
                            player,
                            hold: Hold::BloodSuck,
                        });
                        let hurt = bite_hurt(player);
                        hurts.write(hurt);
                        if hurt_kills(&hurt, dying, *shield, *health, *invincible) {
                            animator.morph_to(anim::FLY, KILLED_PLAYER_MORPH_RATE);
                            brain.mode = MosquitoMode::Realign;
                            brain.state = MosquitoState::Flying;
                            // Sound: start or update EFFECT_BUZZ at the mosquito.
                            continue;
                        }
                        brain.state = MosquitoState::Sucking;
                        brain.victim = Some(player);
                        // Lined up at once (`MoveMosquito_Sucking`), without
                        // the collision.
                    }
                }
            }
            MosquitoState::Sucking | MosquitoState::Dying => {}
        }

        if brain.state == MosquitoState::Sucking {
            let victim = brain
                .victim
                .and_then(|victim| players.get(victim).ok())
                .and_then(|(_, transform, _, _, model, ..)| {
                    model_point(
                        &rigs,
                        model.0,
                        transform,
                        PLAYER_HEAD_JOINT,
                        PLAYER_HEAD_OFFSET,
                    )
                });
            let tip = model_point(&rigs, model.0, &body.transform, HEAD_JOINT, TIP_OFFSET);
            if let (Some(head), Some(tip)) = (victim, tip) {
                body.transform.translation = stinger_in_head(body.transform.translation, tip, head);
            }
            if animator.has_stopped {
                animator.morph_to(anim::FLY_FULL, FULL_MORPH_RATE);
                brain.mode = MosquitoMode::Realign;
                brain.state = MosquitoState::Flying;
                if let Some(victim) = brain.victim.take() {
                    victims.stand_up(victim);
                    releases.write(ReleasePlayer {
                        player: victim,
                        restore_collision: true,
                    });
                }
            }
            // Sound: start or update EFFECT_BUZZ at the mosquito.
            continue;
        }

        let brain = &mut *brain;
        let animator = &mut *animator;
        let victims = &mut victims;
        collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| {
            // `KillMosquito`'s delta is overwritten by `gDelta` when the
            // move ends, so the velocity is left alone here. A mosquito
            // that collides isn't sucking, so there's no bug to stand up,
            // but it is handled all the same.
            if let Some(victim) =
                kill_mosquito(&mut commands, mosquito, brain, Some(animator), shape)
            {
                victims.stand_up(victim);
            }
            false
        });
        // Sound: start or update EFFECT_BUZZ at the mosquito.
    }
}

/// Moves the mosquitoes along their splines, and lets them off to chase a
/// player that comes in range.
///
/// Port of `MoveMosquitoOnSpline` (original/src/Enemies/Enemy_Mosquito.c).
/// The collision box and shadow follow the transform by themselves. The
/// original's check against the Pond's water changes `gCoord`, which this
/// routine doesn't use, so a mosquito on a spline isn't kept above the
/// water; neither is it here.
#[allow(clippy::type_complexity)]
fn move_mosquitoes_on_spline(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut mosquitoes: Query<(
        Entity,
        &mut Transform,
        &mut OnSpline,
        &PreviousPosition,
        &mut MosquitoBrain,
        &EnemyModel,
    )>,
    mut animators: Query<&mut SkeletonAnimator, Without<MosquitoBrain>>,
    players: Query<&Transform, (With<Player>, Without<MosquitoBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (mosquito, mut transform, mut on_spline, previous, mut brain, model) in &mut mosquitoes {
        if brain.state == MosquitoState::Dying {
            continue;
        }
        if let Ok(mut animator) = animators.get_mut(model.0) {
            animator.speed = FLY_ANIM_SPEED;
        }
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;

        if !on_spline.visible {
            // Sound: stop EFFECT_BUZZ.
            continue;
        }
        // Sound: start or update EFFECT_BUZZ at the mosquito.
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), previous.xz(), position);
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y =
            map.floor_height(position.x, position.y) + FLIGHT_HEIGHT + brain.wobble(dt);

        let at = transform.translation;
        let near = nearest_player(at, players.iter().map(|t| t.translation))
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < CHASE_RANGE);
        if near {
            detach_enemy_from_spline(&mut commands.entity(mosquito));
            brain.mode = MosquitoMode::Chase;
            brain.honor_range = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::physics::Velocity;

    const DT: f32 = 1.0 / 60.0;

    fn flat(_: Vec2) -> f32 {
        0.0
    }

    #[test]
    fn a_waiting_mosquito_hovers_at_its_height_and_chases_once_the_player_is_near() {
        let mut brain = MosquitoBrain::default();
        let at = Vec3::new(0.0, FLIGHT_HEIGHT, 0.0);
        let far = Vec2::new(0.0, -1000.0);
        let step = fly_step(&mut brain, 0.0, at, Vec2::ZERO, far, flat, DT);
        assert_eq!(brain.mode, MosquitoMode::Waiting);
        assert!(!step.start_chase);
        assert_eq!(step.velocity, None);
        assert_eq!(step.coord.xz(), Vec2::ZERO);
        let wobble = (WOBBLE_RATE * DT).sin() * WOBBLE_HEIGHT;
        assert!((step.coord.y - (FLIGHT_HEIGHT + wobble)).abs() < 1e-4);

        let near = Vec2::new(0.0, -300.0);
        let step = fly_step(&mut brain, 0.0, at, Vec2::ZERO, near, flat, DT);
        assert_eq!(brain.mode, MosquitoMode::Chase);
        assert!(step.start_chase);
    }

    #[test]
    fn a_chasing_mosquito_flies_at_the_player_and_bites_when_close() {
        let mut brain = MosquitoBrain {
            mode: MosquitoMode::Chase,
            ..default()
        };
        let at = Vec3::new(0.0, FLIGHT_HEIGHT, 0.0);
        // Yaw 0 faces −Z, where the player is.
        let step = fly_step(
            &mut brain,
            0.0,
            at,
            Vec2::ZERO,
            Vec2::new(0.0, -300.0),
            flat,
            DT,
        );
        assert!(!step.bite);
        let velocity = step.velocity.unwrap_or_default();
        assert!((velocity.y + CHASE_SPEED).abs() < 1e-3);
        assert!((step.coord.z + CHASE_SPEED * DT).abs() < 1e-3);

        let step = fly_step(
            &mut brain,
            0.0,
            at,
            Vec2::ZERO,
            Vec2::new(0.0, -150.0),
            flat,
            DT,
        );
        assert!(step.bite);
    }

    #[test]
    fn a_mosquito_too_far_from_home_goes_back_unless_it_left_a_spline() {
        let away = Vec3::new(MAX_RANGE + 100.0, FLIGHT_HEIGHT, 0.0);
        let player = away.xz() + Vec2::new(300.0, 0.0);
        let mut brain = MosquitoBrain {
            mode: MosquitoMode::Chase,
            ..default()
        };
        fly_step(&mut brain, 0.0, away, Vec2::ZERO, player, flat, DT);
        assert_eq!(brain.mode, MosquitoMode::GoHome);

        let mut brain = MosquitoBrain {
            mode: MosquitoMode::Chase,
            honor_range: false,
            ..default()
        };
        fly_step(&mut brain, 0.0, away, Vec2::ZERO, player, flat, DT);
        assert_eq!(brain.mode, MosquitoMode::Chase);

        let mut brain = MosquitoBrain {
            mode: MosquitoMode::GoHome,
            ..default()
        };
        let close = Vec3::new(100.0, FLIGHT_HEIGHT, 0.0);
        fly_step(&mut brain, 0.0, close, Vec2::ZERO, player, flat, DT);
        assert_eq!(brain.mode, MosquitoMode::Waiting);
    }

    #[test]
    fn after_a_bite_it_climbs_back_and_then_flies_home() {
        let mut brain = MosquitoBrain {
            mode: MosquitoMode::Realign,
            ..default()
        };
        let low = Vec3::new(0.0, 100.0, 0.0);
        let step = fly_step(&mut brain, 0.0, low, Vec2::ZERO, Vec2::ZERO, flat, 0.5);
        assert_eq!(step.coord.y, 100.0 + REALIGN_RISE_SPEED * 0.5);
        assert_eq!(brain.mode, MosquitoMode::Realign);

        let high = Vec3::new(0.0, FLIGHT_HEIGHT + 20.0, 0.0);
        let step = fly_step(&mut brain, 0.0, high, Vec2::ZERO, Vec2::ZERO, flat, DT);
        assert_eq!(step.coord, high);
        assert_eq!(brain.mode, MosquitoMode::GoHome);
    }

    #[test]
    fn a_diving_mosquito_drops_ever_faster_toward_the_player() {
        let start = Vec3::new(0.0, 300.0, 0.0);
        let (yaw, coord, velocity) = dive_step(
            0.0,
            start,
            Vec3::new(0.0, -100.0, 0.0),
            Vec2::new(0.0, -500.0),
            0.1,
        );
        assert_eq!(yaw, 0.0);
        assert!((velocity.z + CHASE_SPEED).abs() < 1e-3);
        assert!((velocity.y - (-100.0 - DIVE_ACCEL * 0.1)).abs() < 1e-3);
        assert!((coord - (start + velocity * 0.1)).length() < 1e-3);
    }

    #[test]
    fn the_stinger_goes_into_the_head() {
        let coord = Vec3::new(10.0, 20.0, 30.0);
        let tip = coord + Vec3::new(0.0, -20.0, 70.0);
        let head = Vec3::new(500.0, 60.0, -40.0);
        let moved = stinger_in_head(coord, tip, head);
        assert!((moved + (tip - coord) - head).length() < 1e-4);
    }

    #[test]
    fn the_bite_kills_only_a_weak_player_and_ignores_the_shield() {
        let hurt = bite_hurt(Entity::PLACEHOLDER);
        let none = InvincibleTimer(0.0);
        let shield = ShieldTimer(5.0);
        assert!(hurt_kills(&hurt, false, shield, Health(0.2), none));
        assert!(!hurt_kills(&hurt, false, shield, Health(0.5), none));
        assert!(!hurt_kills(
            &hurt,
            false,
            shield,
            Health(0.1),
            InvincibleTimer(1.0)
        ));
        assert!(!hurt_kills(&hurt, true, shield, Health(0.1), none));
        assert!(!hurt.knock);
    }

    /// A sucking mosquito with its model and its victim.
    fn world_with_mosquito(victim_state: BugState) -> (World, Entity, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<EnemyKicked>>();
        world.init_resource::<Messages<BallHitEnemy>>();
        world.init_resource::<Messages<EnemyKilled>>();
        let model = world.spawn(SkeletonAnimator::default()).id();
        let player = world.spawn((Player, PlayerForm::Bug, victim_state)).id();
        let mosquito = world
            .spawn((
                MosquitoBrain {
                    state: MosquitoState::Sucking,
                    victim: Some(player),
                    ..default()
                },
                Velocity::default(),
                EnemyModel(model),
                Transform::default(),
                Shadows::default(),
                solid_object(
                    vec![COLLISION_BOX],
                    [CollisionKind::Enemy, CollisionKind::Kickable],
                    SolidSides::NOT_TOP,
                ),
            ))
            .id();
        world
            .get_mut::<SkeletonAnimator>(model)
            .expect("the model")
            .set_anim(anim::SUCK);
        (world, mosquito, model, player)
    }

    #[test]
    fn killing_a_sucking_mosquito_stands_the_bug_up_whatever_it_is_doing() {
        let (mut world, mosquito, model, player) = world_with_mosquito(BugState::KnockedOnButt);
        world.write_message(EnemyKilled {
            enemy: mosquito,
            knock: Vec3::ZERO,
        });
        world
            .run_system_once(kill_hurt_mosquitoes)
            .expect("the system runs");
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Stand));
        let brain = world.get::<MosquitoBrain>(mosquito).copied();
        assert_eq!(brain.map(|b| b.state), Some(MosquitoState::Dying));
        assert_eq!(brain.and_then(|b| b.victim), None);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(anim::DEATH)
        );
        let boxes = world.get::<CollisionBoxes>(mosquito).map(|b| b.0.clone());
        assert_eq!(
            boxes,
            Some(vec![CollisionBox {
                bottom: 0.0,
                ..COLLISION_BOX
            }])
        );
    }

    #[test]
    fn a_dying_bug_is_not_stood_up() {
        let (mut world, mosquito, _, player) = world_with_mosquito(BugState::Death);
        world.entity_mut(player).insert(Dying { timer: 1.0 });
        world.write_message(EnemyKilled {
            enemy: mosquito,
            knock: Vec3::ZERO,
        });
        world
            .run_system_once(kill_hurt_mosquitoes)
            .expect("the system runs");
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Death));
    }

    #[test]
    fn the_kick_sends_a_mosquito_flying_and_a_slow_ball_does_nothing() {
        let (mut world, mosquito, _, _) = world_with_mosquito(BugState::BloodSuck);
        world.entity_mut(mosquito).insert(MosquitoBrain::default());
        world.write_message(BallHitEnemy {
            player: Entity::PLACEHOLDER,
            enemy: mosquito,
            ball_velocity: Vec3::X * 1300.0,
            ball_speed: 1300.0,
        });
        world
            .run_system_once(ball_hit_mosquitoes)
            .expect("the system runs");
        let state = |world: &World| world.get::<MosquitoBrain>(mosquito).map(|b| b.state);
        assert_eq!(state(&world), Some(MosquitoState::Flying));

        world.write_message(EnemyKicked {
            player: Entity::PLACEHOLDER,
            enemy: mosquito,
            direction: Vec2::new(0.0, -1.0),
            damage: 0.4,
        });
        world
            .run_system_once(kick_mosquitoes)
            .expect("the system runs");
        assert_eq!(state(&world), Some(MosquitoState::Dying));
        assert_eq!(
            world.get::<Velocity>(mosquito),
            Some(&Velocity(Vec3::new(0.0, KICK_SPEED, -KICK_SPEED)))
        );
    }

    #[test]
    fn a_fast_ball_knocks_a_mosquito_down() {
        let (mut world, mosquito, _, _) = world_with_mosquito(BugState::BloodSuck);
        world.entity_mut(mosquito).insert(MosquitoBrain::default());
        world.write_message(BallHitEnemy {
            player: Entity::PLACEHOLDER,
            enemy: mosquito,
            ball_velocity: Vec3::new(1600.0, 0.0, 0.0),
            ball_speed: 1600.0,
        });
        world
            .run_system_once(ball_hit_mosquitoes)
            .expect("the system runs");
        assert_eq!(
            world.get::<Velocity>(mosquito),
            Some(&Velocity(Vec3::new(800.0, BALL_KNOCK_RISE, 0.0)))
        );
    }

    #[test]
    fn the_pond_has_mosquitoes() {
        use bugdom_formats::rsrc::ResourceFork;
        let path = bugdom_formats::original_data_dir().join("Terrain/Pond.ter.rsrc");
        let fork = ResourceFork::open(&path).expect("terrain file");
        let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
        let on_map = terrain.items.iter().any(|i| i.kind == kind::MOSQUITO);
        let on_splines = terrain
            .splines
            .iter()
            .flat_map(|s| &s.items)
            .any(|i| i.kind == kind::MOSQUITO);
        assert!(on_map || on_splines);
    }
}
