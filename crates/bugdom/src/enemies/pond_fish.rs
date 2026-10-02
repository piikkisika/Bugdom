//! The pond fish: it lurks deep in the pond, follows a swimming player and
//! leaps out of the water to swallow it whole.
//!
//! Port of original/src/Enemies/Enemy_PondFish.c, a map item
//! ([`kind::POND_FISH`]). Nothing collides with the fish (`CType = 0`) and
//! it doesn't call `DoEnemyCollisionDetect`, so it answers no contact
//! message and can't be killed.
//!
//! The original keeps the fish that is eating the player in
//! `gCurrentEatingFish`. Here each fish remembers the player it swallowed
//! ([`PondFishBrain::eaten`]) until that player starts again, so other
//! fish leave that player alone.

use std::collections::HashMap;

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, MAX_ENEMIES, apply_friction,
    move_enemy, nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionBoxes, SolidSides};
use crate::effects::{ParticleGroups, RippleMaker, make_ripple, make_splash};
use crate::items::{ItemSpawn, RegisterItemKind, kind};
use crate::liquids::Underwater;
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_forward, yaw_of};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{KillPlayer, Player, PlayerForm, PlayerModel, PlayerRespawned};
use crate::skeleton::{
    AnimationFlags, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position,
};
use crate::terrain::TerrainMap;

pub struct PondFishPlugin;

impl Plugin for PondFishPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::POND_FISH, add_pond_fish)
            .add_observer(kill_player_in_deleted_fish)
            .add_systems(
                FixedUpdate,
                (forget_respawned_players, move_pond_fish)
                    .chain()
                    .in_set(EnemySystems::Move),
            );
    }
}

/// The most pond fish at once (`MAX_PONDFISH`).
const MAX_PONDFISH: usize = 3;

/// The pond's water surface (`WATER_Y`).
const WATER_Y: f32 = 0.0;
/// The depth a fish waits and swims at (`PONDFISH_Y`).
const PONDFISH_Y: f32 = WATER_Y - 240.0;
/// How deep the water must be for a fish to swim there, in units (the
/// `350.0f` in "make sure not too shallow").
const MIN_DEPTH: f32 = 350.0;

/// How close a swimming player must be for a fish to chase it, and how far
/// before it gives up, in units (`PONDFISH_CHASE_DIST`).
const CHASE_DIST: f32 = 1900.0;
/// Chasing speed, in units per second (`PONDFISH_CHASESPEED`).
const CHASE_SPEED: f32 = 700.0;
/// How close a chasing fish must be to leap at the player, in units
/// (`PONDFISH_ATTACK_DIST`).
const ATTACK_DIST: f32 = 400.0;
/// How far off the player a fish may still aim to leap at it, in radians.
const ATTACK_AIM: f32 = 0.8;
/// The leap's upward speed, in units per second (`PONDFISH_JUMPSPEED`).
const JUMP_SPEED: f32 = 2400.0;
/// Gravity during the leap, in units per second squared.
const JUMP_GRAVITY: f32 = 3400.0;
/// How strongly an aimed leap steers the mouth at the player: its
/// horizontal velocity is the gap times this, per second.
const JUMP_STEER: f32 = 15.0;
/// Friction while waiting for the leap to start, per frame at 60 fps
/// (`ApplyFrictionToDeltas(100.0, ...)`).
const JUMP_WAIT_FRICTION_PER_FRAME: f32 = 100.0;
/// Turn speed, in radians per second (`PONDFISH_TURN_SPEED`).
const TURN_SPEED: f32 = 1.8;
/// The shortest wait before a fish leaps again or at random, in seconds,
/// and the most random time added to it.
const LEAP_DELAY_MIN: f32 = 2.0;
const LEAP_DELAY_RANDOM: f32 = 2.0;
/// Seconds a swallowed player lasts in the mouth (`EatenTimer = 3`).
const EAT_TIME: f32 = 3.0;

/// `PONDFISH_HEALTH`.
const HEALTH: f32 = 1.0;
/// `PONDFISH_DAMAGE`; nothing touches the fish, so it is never dealt.
const DAMAGE: f32 = 0.04;
/// `PONDFISH_SCALE`, and the most random scale added to it.
const SCALE: f32 = 2.0;
const SCALE_RANDOM: f32 = 0.3;

/// The animations (`PONDFISH_ANIM_*`).
const ANIM_WAIT: usize = 0;
const ANIM_JUMP_ATTACK: usize = 1;
const ANIM_MOUTHFULL: usize = 2;
/// How fast it morphs into the leap, and back out of it, per second.
const JUMP_MORPH_RATE: f32 = 6.0;
const LAND_MORPH_RATE: f32 = 4.0;
/// The animation flag the leap sets when the fish should push off
/// (`JumpNow`, `Flag[0]`).
const JUMP_NOW_FLAG: usize = 0;

/// The head joint (`PONDFISH_JOINT_HEAD`) and the mouth in its space
/// (`gPondFishMouthOff`).
const HEAD_JOINT: usize = 4;
const MOUTH_OFFSET: Vec3 = Vec3::new(0.0, -17.0, -40.0);
/// Half the size of the cube around the mouth that swallows the player.
const MOUTH_REACH: f32 = 15.0;
/// The bug's pelvis joint (`BUG_LIMB_NUM_PELVIS`), which an aimed leap
/// steers at.
const PLAYER_PELVIS_JOINT: usize = 0;

/// Ripples: the scale of a chasing fish's wake, how often it makes one in
/// seconds, and the two rings of a leap or a splashdown.
const WAKE_RIPPLE_SCALE: f32 = 2.0;
const WAKE_RIPPLE_INTERVAL: f32 = 0.2;
const LEAP_RIPPLE_SCALES: [f32; 2] = [7.0, 5.0];
/// Splashes: their force and sound volume, and how far below the surface
/// the leap's and the splashdown's start.
const SPLASH_FORCE: f32 = 1.0;
const SPLASH_VOLUME: f32 = 4.0;
const LEAP_SPLASH_DEPTH: f32 = 50.0;
const LANDING_SPLASH_DEPTH: f32 = 100.0;

/// What a fish is doing; it follows its animation, as the original
/// indexes its move table with `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PondFishState {
    /// Waiting or chasing under water (`MovePondFish_Waiting`).
    #[default]
    Waiting,
    /// Leaping (`MovePondFish_JumpAttack`).
    JumpAttack,
    /// Back under water with the player in its mouth; it moves as when
    /// waiting.
    MouthFull,
}

/// Whether a fish goes after the player (`PONDFISH_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PondFishMode {
    /// Turning to face the player, and leaping now and then.
    #[default]
    Wait,
    Chase,
}

/// A pond fish's own state.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct PondFishBrain {
    pub state: PondFishState,
    pub mode: PondFishMode,
    /// Seconds since the last wake ripple (`RippleTimer`).
    pub ripple_timer: f32,
    /// It can't leap at the player until this runs out (`AttackTimer`).
    pub attack_timer: f32,
    /// Seconds until a waiting fish leaps anyway (`RandomJumpTimer`).
    pub random_jump_timer: f32,
    /// Seconds until the swallowed player dies (`EatenTimer`).
    pub eaten_timer: f32,
    /// In the air (`IsJumping`).
    pub is_jumping: bool,
    /// Chewing on the player, until it dies (`EatPlayer`).
    pub eating: bool,
    /// The leap steers at the player (`TweakJumps`); random leaps don't.
    pub tweak_jumps: bool,
    /// The player this fish swallowed, until it starts again
    /// (`gCurrentEatingFish`).
    pub eaten: Option<Entity>,
}

impl PondFishBrain {
    /// Counts the random leap timer down and, when it runs out, sets it
    /// again and returns true. Port of "see if do random jump".
    fn random_jump_due(&mut self, dt: f32, random: &mut GameRandom) -> bool {
        self.random_jump_timer -= dt;
        if self.random_jump_timer < 0.0 {
            self.random_jump_timer = LEAP_DELAY_MIN + random.next_f32() * LEAP_DELAY_RANDOM;
            true
        } else {
            false
        }
    }

    /// The end of a chasing fish's move: it gives up on a player that got
    /// too far, and returns whether to leap at one that is close and in
    /// front. `free` is whether the player swims and isn't being eaten.
    fn chase_attack(&mut self, free: bool, dist: f32, aim: f32) -> bool {
        if !free {
            return false;
        }
        if dist > CHASE_DIST {
            self.mode = PondFishMode::Wait;
            false
        } else {
            dist < ATTACK_DIST && aim < ATTACK_AIM && self.attack_timer < 0.0
        }
    }

    /// Counts the swallowed player's time down, and returns the player once
    /// it is up. Port of `PondFish_ContinueEatingPlayer`.
    fn chew(&mut self, dt: f32) -> Option<Entity> {
        self.eaten_timer -= dt;
        if self.eaten_timer < 0.0 {
            self.eating = false;
            self.eaten
        } else {
            None
        }
    }

    /// Counts the wake ripple timer up while it chases under water, and
    /// returns whether to make a ripple now. The ripple part of
    /// `UpdatePondFish`.
    fn wake_ripple_due(&mut self, dt: f32) -> bool {
        if self.mode == PondFishMode::Wait || self.is_jumping {
            return false;
        }
        self.ripple_timer += dt;
        if self.ripple_timer > WAKE_RIPPLE_INTERVAL {
            self.ripple_timer = 0.0;
            true
        } else {
            false
        }
    }
}

/// Port of `AddEnemy_PondFish` (original/src/Enemies/Enemy_PondFish.c).
fn add_pond_fish(
    In(spawn): In<ItemSpawn>,
    mut enemies: EnemySpawner,
    mut random: ResMut<GameRandom>,
) -> bool {
    if enemies.counts().total() >= MAX_ENEMIES
        || enemies.counts().of_kind(EnemyKind::PondFish) >= MAX_PONDFISH
    {
        return false;
    }
    let scale = SCALE + random.next_f32() * SCALE_RANDOM;
    let floor = enemies
        .map()
        .floor_height(spawn.position.x, spawn.position.y);
    let mut skeleton = EnemySkeleton::new(
        EnemyKind::PondFish,
        SkeletonType::PondFish,
        spawn.position,
        scale,
    )
    .from_item(spawn.index)
    .anim(ANIM_WAIT)
    .foot_offset(floor - PONDFISH_Y)
    .health(HEALTH)
    .damage(DAMAGE)
    .solid(SolidSides::NONE);
    // "Nothing ever collides against the fish."
    skeleton.kinds = avian3d::prelude::LayerMask::NONE;
    let Some(fish) = enemies.spawn(skeleton) else {
        return false;
    };
    enemies
        .commands()
        .entity(fish)
        .insert(PondFishBrain::default());
    true
}

/// A player that starts again is no longer in any fish's mouth.
///
/// Port of the `gCurrentEatingFish = NULL` in `ResetPlayer`
/// (original/src/Player/MyGuy.c). The original leaves `EatPlayer` set on
/// the fish, which can't matter there as the player can't die any other
/// way while it is eaten; it is cleared too here, so that a fish can't
/// kill the player after it has started again.
fn forget_respawned_players(
    mut respawned: MessageReader<PlayerRespawned>,
    mut fish: Query<&mut PondFishBrain>,
) {
    for PlayerRespawned(player) in respawned.read() {
        for mut brain in &mut fish {
            if brain.eaten == Some(*player) {
                brain.eaten = None;
                brain.eating = false;
            }
        }
    }
}

/// A fish that goes out of range with the player in its mouth kills it.
///
/// Port of the `gCurrentEatingFish` check in `MovePondFish`
/// (original/src/Enemies/Enemy_PondFish.c).
fn kill_player_in_deleted_fish(
    remove: On<Remove, PondFishBrain>,
    fish: Query<&PondFishBrain>,
    mut kills: MessageWriter<KillPlayer>,
) {
    if let Some(player) = fish.get(remove.entity).ok().and_then(|b| b.eaten) {
        kills.write(KillPlayer {
            player,
            change_anims: false,
        });
    }
}

/// A player as the fish see it.
#[derive(Debug, Clone, Copy)]
struct Prey {
    entity: Entity,
    position: Vec3,
    /// Where an aimed leap steers the mouth: the bug's pelvis, or the
    /// ball's centre.
    aim_point: Vec3,
    swimming: bool,
    is_ball: bool,
}

/// The cube around the mouth that swallows the player
/// (`DoSimpleBoxCollisionAgainstPlayer` in `SeeIfFishEatsPlayer`).
fn mouth_box(mouth: Vec3) -> CollisionBox {
    CollisionBox::new(
        mouth.y + MOUTH_REACH,
        mouth.y - MOUTH_REACH,
        mouth.x - MOUTH_REACH,
        mouth.x + MOUTH_REACH,
        mouth.z + MOUTH_REACH,
        mouth.z - MOUTH_REACH,
    )
}

/// Makes a leap's or a splashdown's splash and its two ripples.
fn splash(
    commands: &mut Commands,
    ripples: &mut RippleMaker,
    particles: &mut ParticleGroups,
    random: &mut GameRandom,
    at: Vec3,
    depth: f32,
) {
    make_splash(
        particles,
        random,
        Vec3::new(at.x, WATER_Y - depth, at.z),
        SPLASH_FORCE,
        SPLASH_VOLUME,
    );
    for scale in LEAP_RIPPLE_SCALES {
        make_ripple(commands, ripples, Vec3::new(at.x, WATER_Y, at.z), scale);
    }
}

/// Starts a leap (the `attack:` label in `MovePondFish_Waiting`).
fn start_leap(
    brain: &mut PondFishBrain,
    animator: &mut SkeletonAnimator,
    flags: &mut AnimationFlags,
    tweak: bool,
) {
    brain.tweak_jumps = tweak;
    animator.morph_to(ANIM_JUMP_ATTACK, JUMP_MORPH_RATE);
    brain.state = PondFishState::JumpAttack;
    flags.0[JUMP_NOW_FLAG] = false;
    brain.is_jumping = false;
}

/// Whether the floor here leaves the fish too little water.
fn too_shallow(map: &TerrainMap, at: Vec3) -> bool {
    map.floor_height(at.x, at.z) + MIN_DEPTH > WATER_Y
}

/// The parts of the fish's and the players' models that the fish read.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SkeletonAnimator,
        &'static mut AnimationFlags,
        Option<&'static SkeletonRig>,
        &'static Transform,
    ),
    Without<PondFishBrain>,
>;

/// Where a joint point of a model is, given its root's transform.
fn model_point(
    models: &ModelQuery,
    model: Entity,
    root: &Transform,
    joint: usize,
    offset: Vec3,
) -> Option<Vec3> {
    let (_, _, rig, model_transform) = models.get(model).ok()?;
    let base = Affine3A::from_rotation_translation(root.rotation, root.translation)
        * model_transform.compute_affine();
    joint_position(rig?, joint, offset, base)
}

/// Moves the pond fish: waiting, chasing, leaping and eating.
///
/// Port of `MovePondFish`, `MovePondFish_Waiting`,
/// `MovePondFish_JumpAttack`, `UpdatePondFish`, `SeeIfFishEatsPlayer` and
/// `PondFish_ContinueEatingPlayer`
/// (original/src/Enemies/Enemy_PondFish.c). Going out of range is the
/// items' `DespawnOutOfRange`. Each fish goes for the nearest player.
#[allow(clippy::too_many_arguments)]
fn move_pond_fish(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut random: ResMut<GameRandom>,
    mut particles: ResMut<ParticleGroups>,
    mut ripples: RippleMaker,
    mut fish: Query<(
        Entity,
        &mut Transform,
        &mut Velocity,
        &PreviousPosition,
        &mut PondFishBrain,
        &EnemyModel,
    )>,
    mut models: ModelQuery,
    players: Query<
        (
            Entity,
            &Transform,
            &CollisionBoxes,
            &PlayerForm,
            Has<Underwater>,
            &PlayerModel,
        ),
        (With<Player>, Without<PondFishBrain>),
    >,
    mut kills: MessageWriter<KillPlayer>,
) {
    let dt = time.delta_secs();
    let prey: Vec<Prey> = players
        .iter()
        .map(|(entity, transform, _, form, swimming, model)| {
            let is_ball = *form == PlayerForm::Ball;
            let pelvis = (!is_ball)
                .then(|| model_point(&models, model.0, transform, PLAYER_PELVIS_JOINT, Vec3::ZERO))
                .flatten();
            Prey {
                entity,
                position: transform.translation,
                aim_point: pelvis.unwrap_or(transform.translation),
                swimming,
                is_ball,
            }
        })
        .collect();
    // Which fish has each swallowed player (`gCurrentEatingFish`).
    let mut eaters: HashMap<Entity, Entity> = fish
        .iter()
        .filter_map(|(entity, .., brain, _)| brain.eaten.map(|player| (player, entity)))
        .collect();

    for (entity, mut transform, mut velocity, previous, mut brain, model) in &mut fish {
        brain.attack_timer -= dt;

        let mut coord = transform.translation;
        let mut yaw = yaw_of(transform.rotation);
        let mut v = **velocity;
        let target_at = nearest_player(coord, prey.iter().map(|p| p.position));
        let target = target_at.and_then(|at| prey.iter().find(|p| p.position == at).copied());
        let target_xz = target.map_or(coord.xz(), |p| p.position.xz());
        let eaten_by = target.and_then(|p| eaters.get(&p.entity).copied());

        // Another fish has the player.
        if eaten_by.is_some_and(|f| f != entity) {
            brain.mode = PondFishMode::Wait;
        }
        let free = target.is_some_and(|p| p.swimming) && eaten_by.is_none();
        // The mouth as the last frame drew it (`FindCoordOnJoint`).
        let mouth = model_point(&models, model.0, &transform, HEAD_JOINT, MOUTH_OFFSET);

        let Ok((mut animator, mut flags, ..)) = models.get_mut(model.0) else {
            continue;
        };

        match brain.state {
            PondFishState::Waiting | PondFishState::MouthFull => match brain.mode {
                PondFishMode::Wait => {
                    (yaw, _) = turn_toward(yaw, coord.xz(), target_xz, TURN_SPEED * dt);
                    if brain.random_jump_due(dt, &mut random) {
                        start_leap(&mut brain, &mut animator, &mut flags, false);
                    } else if free && quick_distance(target_xz, coord.xz()) < CHASE_DIST {
                        brain.mode = PondFishMode::Chase;
                    }
                }
                PondFishMode::Chase => {
                    let aim;
                    (yaw, aim) = turn_toward(yaw, coord.xz(), target_xz, TURN_SPEED * dt);
                    let forward = yaw_forward(yaw) * CHASE_SPEED;
                    v.x = forward.x;
                    v.z = forward.y;
                    move_enemy(&mut coord, v, dt);
                    if too_shallow(&map, coord) {
                        coord.x = previous.x;
                        coord.z = previous.z;
                    }
                    let dist = quick_distance(target_xz, coord.xz());
                    if brain.chase_attack(free, dist, aim) {
                        start_leap(&mut brain, &mut animator, &mut flags, true);
                    }
                }
            },
            PondFishState::JumpAttack => {
                (yaw, _) = turn_toward(yaw, coord.xz(), target_xz, TURN_SPEED * dt);
                if std::mem::take(&mut flags.0[JUMP_NOW_FLAG]) {
                    brain.is_jumping = true;
                    v = Vec3::new(0.0, JUMP_SPEED, 0.0);
                    move_enemy(&mut coord, v, dt);
                    splash(
                        &mut commands,
                        &mut ripples,
                        &mut particles,
                        &mut random,
                        coord,
                        LEAP_SPLASH_DEPTH,
                    );
                } else if brain.is_jumping {
                    if brain.tweak_jumps
                        && let (Some(mouth), Some(prey)) = (mouth, target)
                    {
                        let gap = prey.aim_point - mouth;
                        v.x = gap.x * JUMP_STEER;
                        v.z = gap.z * JUMP_STEER;
                    }
                    v.y -= JUMP_GRAVITY * dt;
                    move_enemy(&mut coord, v, dt);
                    if too_shallow(&map, coord) {
                        coord.x = previous.x;
                        coord.z = previous.z;
                    }
                    // Splashdown.
                    if coord.y < PONDFISH_Y && v.y < 0.0 {
                        coord.y = PONDFISH_Y;
                        v = Vec3::ZERO;
                        if brain.eating {
                            animator.morph_to(ANIM_MOUTHFULL, LAND_MORPH_RATE);
                            brain.state = PondFishState::MouthFull;
                        } else {
                            animator.morph_to(ANIM_WAIT, LAND_MORPH_RATE);
                            brain.state = PondFishState::Waiting;
                        }
                        brain.is_jumping = false;
                        splash(
                            &mut commands,
                            &mut ripples,
                            &mut particles,
                            &mut random,
                            coord,
                            LANDING_SPLASH_DEPTH,
                        );
                        brain.attack_timer = LEAP_DELAY_MIN + random.next_f32() * LEAP_DELAY_RANDOM;
                    }
                } else {
                    apply_friction(&mut v, per_frame_friction(JUMP_WAIT_FRICTION_PER_FRAME), dt);
                    move_enemy(&mut coord, v, dt);
                }

                // `SeeIfFishEatsPlayer`: only the bug, and only one not
                // eaten already.
                if !brain.eating
                    && let Some(mouth) = mouth
                {
                    let bite = mouth_box(mouth);
                    let caught = players.iter().find(|(player, transform, boxes, ..)| {
                        let is_ball = prey.iter().any(|p| p.entity == *player && p.is_ball);
                        !is_ball
                            && !eaters.contains_key(player)
                            && boxes
                                .0
                                .iter()
                                .any(|b| b.at(transform.translation).overlaps(&bite))
                    });
                    if let Some((player, ..)) = caught {
                        eaters.insert(player, entity);
                        brain.eaten = Some(player);
                        brain.eating = true;
                        brain.eaten_timer = EAT_TIME;
                        // The player's side of being eaten (its
                        // `PLAYER_ANIM_BEINGEATEN`, `CType = 0` and
                        // `MovePlayerBug_BeingEaten`) isn't ported yet.
                        if let Some(player) = brain.chew(dt) {
                            kills.write(KillPlayer {
                                player,
                                change_anims: false,
                            });
                        }
                    }
                }
            }
        }

        // `UpdatePondFish`.
        if brain.eating
            && let Some(player) = brain.chew(dt)
        {
            kills.write(KillPlayer {
                player,
                change_anims: false,
            });
        }
        if brain.wake_ripple_due(dt) {
            make_ripple(
                &mut commands,
                &mut ripples,
                Vec3::new(coord.x, WATER_Y, coord.z),
                WAKE_RIPPLE_SCALE,
            );
        }

        transform.translation = coord;
        transform.rotation = Quat::from_rotation_y(yaw);
        **velocity = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_waiting_fish_leaps_at_once_and_then_every_few_seconds() {
        let mut brain = PondFishBrain::default();
        let mut random = GameRandom::default();
        assert!(brain.random_jump_due(DT, &mut random));
        let delay = brain.random_jump_timer;
        assert!((LEAP_DELAY_MIN..=LEAP_DELAY_MIN + LEAP_DELAY_RANDOM).contains(&delay));
        assert!(!brain.random_jump_due(delay - 0.01, &mut random));
        assert!(brain.random_jump_due(0.02, &mut random));
    }

    #[test]
    fn a_chasing_fish_leaps_only_when_close_aimed_and_rested() {
        let mut brain = PondFishBrain {
            mode: PondFishMode::Chase,
            attack_timer: -0.1,
            ..default()
        };
        assert!(brain.chase_attack(true, 300.0, 0.1));
        assert!(!brain.chase_attack(true, 300.0, 1.0));
        assert!(!brain.chase_attack(true, 500.0, 0.1));
        // Not swimming, or eaten already: it keeps chasing without leaping.
        assert!(!brain.chase_attack(false, 300.0, 0.1));
        brain.attack_timer = 1.0;
        assert!(!brain.chase_attack(true, 300.0, 0.1));
        assert_eq!(brain.mode, PondFishMode::Chase);
        // Too far: it gives up.
        assert!(!brain.chase_attack(true, CHASE_DIST + 1.0, 0.1));
        assert_eq!(brain.mode, PondFishMode::Wait);
    }

    #[test]
    fn a_swallowed_player_dies_after_the_eat_time() {
        let player = Entity::from_raw_u32(7).expect("a valid index");
        let mut brain = PondFishBrain {
            eating: true,
            eaten: Some(player),
            eaten_timer: EAT_TIME,
            ..default()
        };
        let ticks = (0..400)
            .position(|_| brain.chew(DT).is_some())
            .expect("the player dies");
        assert_eq!(ticks, 180);
        assert!(!brain.eating);
        // The fish still remembers whom it ate, until that player starts
        // again.
        assert_eq!(brain.eaten, Some(player));
    }

    #[test]
    fn only_a_chasing_fish_under_water_leaves_a_wake() {
        let mut brain = PondFishBrain::default();
        assert!(!(0..60).any(|_| brain.wake_ripple_due(DT)));
        brain.mode = PondFishMode::Chase;
        assert_eq!((0..60).filter(|_| brain.wake_ripple_due(DT)).count(), 4);
        brain.is_jumping = true;
        assert!(!(0..60).any(|_| brain.wake_ripple_due(DT)));
    }

    #[test]
    fn the_mouth_box_surrounds_the_mouth() {
        let bite = mouth_box(Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(bite, CollisionBox::new(35.0, 5.0, -5.0, 25.0, 45.0, 15.0));
    }
}
