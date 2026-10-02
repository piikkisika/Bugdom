//! The flying bee: it hovers above and around the player, and dives at it
//! to sting. A bee dies when it stings (the player's spiked contact), and
//! to a fast ball.
//!
//! Port of original/src/Enemies/Enemy_Bee_Flying.c. Bees come from map
//! items ([`kind::FLYING_BEE`], `AddEnemy_FlyingBee`) on the Forest and
//! Hive levels, and from the hives that release them ([`make_flying_bee`],
//! `MakeFlyingBee`).
//!
//! The original dispatches on the bee's animation (`myMoveTable[AnimNum]`);
//! here that is [`FlyingBeeState`]. A bee is spiked (`CTYPE_SPIKED`) only
//! while it dives, so the player's collision hurts the player and sends
//! `TouchedEnemy { spiked: true }`, which kills the bee.
//!
//! The player riding the dragonfly (`gCurrentDragonFly`) changes how bees
//! keep their distance and dive. The ride isn't ported yet, so
//! [`riding_dragonfly`] is always false.

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled,
    EnemyKind, EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, ORIGINAL_FRAME_RATE,
    TouchedEnemy, apply_friction, death_enemy_collision_mask, default_enemy_collision_mask,
    move_enemy, nearest_player, per_frame_friction,
};
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroups, ParticleKind, ParticleTexture,
};
use crate::items::pickups::DetonatorsBlown;
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_of};
use crate::player::{Player, PlayerForm};
use crate::skeleton::{SkeletonAnimator, SkeletonType};
use crate::terrain::{LayerKind, TerrainMap};

pub struct FlyingBeePlugin;

impl Plugin for FlyingBeePlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::FLYING_BEE, add_flying_bee)
            .add_systems(
                FixedUpdate,
                (
                    kick_flying_bees.in_set(EnemySystems::Kicked),
                    // The player's collision stings and hits before the
                    // bees move, as in the original's frame. The stings go
                    // first, so the ball hit of the same contact finds the
                    // bee dead already.
                    (sting_flying_bees, ball_hit_flying_bees, move_flying_bees)
                        .chain()
                        .in_set(EnemySystems::Move),
                    kill_hurt_flying_bees.in_set(EnemySystems::Killed),
                ),
            );
    }
}

/// `LEVEL_NUM_FLIGHT`, where killed bees come back.
const FLIGHT_LEVEL: usize = 4;
/// `LEVEL_NUM_HIVE`, which allows fewer bees and keys some to detonators.
const HIVE_LEVEL: usize = 5;

/// The most bees counted at once on the Hive level, and for
/// [`make_flying_bee`] (`MAX_FLYINGBEE`).
const MAX_FLYINGBEE: usize = 4;
/// The most bees counted at once from map items elsewhere
/// (`MAX_FLYINGBEE2`).
const MAX_FLYINGBEE2: usize = 8;

/// How close the player must be, in units, for a bee to follow it
/// (`FLYINGBEE_CHASE_RANGE`).
const CHASE_RANGE: f32 = 2000.0;
/// How close the player must be, in x and z, for a bee to dive
/// (`FLYINGBEE_ATTACK_RANGE`); on the dragonfly, `FLYINGBEE_ATTACK_RANGE2`.
const ATTACK_RANGE: f32 = 450.0;
const ATTACK_RANGE_DRAGONFLY: f32 = 400.0;
/// How far above the player a bee must be to dive, in units: more than the
/// first and less than the second (the numbers in `MoveFlyingBee_Flying`).
const DIVE_HEIGHT: (f32, f32) = (200.0, 800.0);
const DIVE_HEIGHT_DRAGONFLY: (f32, f32) = (50.0, 900.0);
/// How much of its horizontal velocity a bee keeps when it starts a dive.
const DIVE_START_SLOWDOWN: f32 = 0.4;
const DIVE_START_SLOWDOWN_DRAGONFLY: f32 = 0.3;
/// A diving bee's horizontal acceleration toward the player, in units per
/// second squared (`DIVE_MOVE_SPEED`, `DIVE_MOVE_SPEED2`).
const DIVE_MOVE_SPEED: f32 = 500.0;
const DIVE_MOVE_SPEED_DRAGONFLY: f32 = 1100.0;
/// A diving bee's downward acceleration, in units per second squared.
const DIVE_GRAVITY: f32 = 1100.0;
const DIVE_GRAVITY_DRAGONFLY: f32 = 800.0;
/// A dive ends this close to the floor, in units.
const DIVE_MIN_HEIGHT: f32 = 50.0;
/// How much of its velocity a bee keeps when a dive ends.
const DIVE_END_SLOWDOWN: f32 = 0.5;

/// How fast a bee closes on where it wants to be, per second
/// (`BEE_ACCEL`).
const BEE_ACCEL: f32 = 1.7;
/// The most of the gap in x or z a bee closes at [`BEE_ACCEL`], in units
/// (the 300 in `MoveFlyingBee_Flying`).
const MAX_CHASE_GAP: f32 = 300.0;
/// How far from the player a bee's height starts to grow, in units
/// (`BEE_CLOSEST`).
const BEE_CLOSEST: f32 = 10.0;
/// How much higher a bee flies per unit it is from the player
/// (`BEE_HEIGHT_FACTOR`).
const BEE_HEIGHT_FACTOR: f32 = 0.17;
/// How far above the player's feet a bee flies at the closest, in units
/// (`BEE_MINY`), and above the ball (`BEE_MINY2`).
const BEE_MINY: f32 = 170.0;
const BEE_MINY_BALL: f32 = 40.0;
/// How much farther a bee keeps from a player on the dragonfly, in units.
const DRAGONFLY_EXTRA_DISTANCE: f32 = 100.0;
/// A bee keeps this far from the player: at least the first and up to the
/// first plus the second, at random (`DistFromMe`).
const DIST_FROM_PLAYER_MIN: f32 = 100.0;
const DIST_FROM_PLAYER_RANGE: f32 = 300.0;
/// How far below the ceiling a bee stays, in units.
const CEILING_CLEARANCE: f32 = 100.0;
/// How far above the floor a bee stays, in units.
const FLOOR_CLEARANCE: f32 = 50.0;
/// Turn speeds, in radians per second, while flying and diving.
const FLY_TURN_SPEED: f32 = 3.0;
const DIVE_TURN_SPEED: f32 = 5.0;

/// How fast a falling bee drops, in units per second squared.
const FALL_GRAVITY: f32 = 800.0;
/// Friction on a dead bee on the ground, per frame at 60 fps
/// (`ApplyFrictionToDeltas(60.0, ...)`).
const DEATH_FRICTION_PER_FRAME: f32 = 60.0;

/// How fast the ball must go to knock a bee down, in units per second
/// (`FLYINGNBEE_KNOCKDOWN_SPEED`; the unused `FLYINGBEE_KNOCKDOWN_SPEED`
/// is 1400).
const KNOCKDOWN_SPEED: f32 = 1100.0;

/// `FLYINGBEE_HEALTH`.
const HEALTH: f32 = 1.0;
/// What a sting does to the player (`FLYINGBEE_DAMAGE`).
const DAMAGE: f32 = 0.1;
/// `FLYINGBEE_SCALE`.
const SCALE: f32 = 0.8;
/// How high above the floor a bee from a map item starts, in units: on the
/// hive levels, elsewhere by default, and per unit of the item's first
/// parameter.
const HIVE_START_HEIGHT: f32 = 200.0;
const DEFAULT_START_HEIGHT: f32 = 600.0;
const START_HEIGHT_PER_PARAM: f32 = 100.0;
/// Shadow size (`AttachShadowToObject(newObj, 8, 8, false)`).
const SHADOW_SCALE: f32 = 8.0;
/// The collision box (`SetObjectCollisionBounds(newObj, 90,-50,-90,90,90,-90)`).
const COLLISION_BOX: CollisionBox = CollisionBox::new(90.0, -50.0, -90.0, 90.0, 90.0, -90.0);

/// The animations (`FLYINGBEE_ANIM_*`).
mod anim {
    pub const FLY: usize = 0;
    pub const DIVE: usize = 1;
    pub const FALL: usize = 2;
    pub const DEATH: usize = 3;
}
/// How much of each blend happens per second: into a dive, back to flying,
/// into the fall, and into lying dead.
const DIVE_MORPH_RATE: f32 = 8.0;
const FLY_MORPH_RATE: f32 = 3.0;
const FALL_MORPH_RATE: f32 = 5.0;
const DEATH_MORPH_RATE: f32 = 8.0;

/// How many sparks a dying bee throws.
const KILL_SPARKS: usize = 60;
/// The sparks' largest speed in each direction, in units per second (the
/// whole width).
const KILL_SPARK_SPEED: f32 = 1400.0;
/// The white sparks of a dying bee (`KillFlyingBee`).
const KILL_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::BOUNCE,
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 20.0,
    decay_rate: 0.8,
    fade_rate: 0.0,
    texture: ParticleTexture::YellowBall,
};

/// What a bee is doing; it follows its animation, as the original indexes
/// its move table with `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FlyingBeeState {
    /// Hovering near the player (`MoveFlyingBee_Flying`).
    #[default]
    Flying,
    /// Diving at the player, spiked (`MoveFlyingBee_Diving`).
    Diving,
    /// Killed, dropping (`MoveFlyingBee_Fall`).
    Falling,
    /// Lying dead until out of view (`MoveFlyingBee_Dead`).
    Dead,
}

impl FlyingBeeState {
    fn is_dying(self) -> bool {
        matches!(self, Self::Falling | Self::Dead)
    }
}

/// A flying bee's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FlyingBeeBrain {
    pub state: FlyingBeeState,
    /// How far from the player it hovers, in units (`DistFromMe`).
    pub dist_from_player: f32,
}

impl FlyingBeeBrain {
    fn new(random: &mut GameRandom) -> Self {
        Self {
            state: FlyingBeeState::Flying,
            dist_from_player: random.next_f32() * DIST_FROM_PLAYER_RANGE + DIST_FROM_PLAYER_MIN,
        }
    }
}

/// Whether the player is riding the dragonfly (`gCurrentDragonFly`).
/// The ride isn't ported yet.
fn riding_dragonfly() -> bool {
    false
}

/// A bee's skeleton at `position`, as both makers set it up.
fn flying_bee_skeleton(position: Vec2) -> EnemySkeleton {
    EnemySkeleton::new(
        EnemyKind::FlyingBee,
        SkeletonType::FlyingBee,
        position,
        SCALE,
    )
    .anim(anim::FLY)
    .health(HEALTH)
    .damage(DAMAGE)
    .solid(SolidSides::NOT_TOP)
    .collision_box(COLLISION_BOX)
    .shadow(SHADOW_SCALE)
}

/// How high above the floor a bee from a map item starts, from the level
/// type and the item's first parameter.
fn start_height(level_type: LevelType, param: u8) -> f32 {
    if level_type == LevelType::Hive {
        HIVE_START_HEIGHT
    } else if param == 0 {
        DEFAULT_START_HEIGHT
    } else {
        f32::from(param) * START_HEIGHT_PER_PARAM
    }
}

/// Port of `AddEnemy_FlyingBee` (original/src/Enemies/Enemy_Bee_Flying.c).
/// Like the original, it checks only the bees, not the total of enemies.
/// On the Hive level a bee whose fourth parameter has bit 0 set waits for
/// the detonator its second parameter names.
fn add_flying_bee(
    In(spawn): In<ItemSpawn>,
    mut enemies: EnemySpawner,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
    mut random: ResMut<GameRandom>,
) -> bool {
    let on_hive = **level == HIVE_LEVEL;
    let max = if on_hive {
        MAX_FLYINGBEE
    } else {
        MAX_FLYINGBEE2
    };
    if enemies.counts().of_kind(EnemyKind::FlyingBee) >= max {
        return false;
    }
    let [height_param, detonator, _, key_flags] = spawn.params;
    if on_hive && key_flags & 1 != 0 && !blown.is_blown(detonator) {
        return false;
    }
    let height = start_height(level.def().level_type, height_param);
    let Some(bee) = enemies.spawn(
        flying_bee_skeleton(spawn.position)
            .from_item(spawn.index)
            .foot_offset(-height),
    ) else {
        return false;
    };
    enemies
        .commands()
        .entity(bee)
        .insert(FlyingBeeBrain::new(&mut random));
    true
}

/// Makes a bee at `position` and returns it; `None` if there are already
/// [`MAX_FLYINGBEE`] bees, or the level has no bee skeleton. Unlike map
/// items, it doesn't check the total of enemies, and it starts spiked.
///
/// Port of `MakeFlyingBee` (original/src/Enemies/Enemy_Bee_Flying.c),
/// which the hives call.
pub fn make_flying_bee(
    enemies: &mut EnemySpawner,
    random: &mut GameRandom,
    position: Vec3,
) -> Option<Entity> {
    if enemies.counts().of_kind(EnemyKind::FlyingBee) >= MAX_FLYINGBEE {
        return None;
    }
    // The spawner puts the origin `foot_offset` below the floor.
    let floor = enemies.map().floor_height(position.x, position.z);
    let bee = enemies.spawn(
        flying_bee_skeleton(position.xz())
            .foot_offset(floor - position.y)
            .with_kinds(CollisionKind::Spiked),
    )?;
    enemies
        .commands()
        .entity(bee)
        .insert(FlyingBeeBrain::new(random));
    Some(bee)
}

/// The player a bee goes for (`gMyCoord`, `gPlayerObj`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct BeeTarget {
    /// Its origin (`gMyCoord`).
    position: Vec3,
    /// Its collision box's bottom, relative to its origin (`BottomOff`).
    bottom: f32,
    /// In ball form (`gPlayerMode == PLAYER_MODE_BALL`).
    ball: bool,
    /// Riding the dragonfly (`gCurrentDragonFly`).
    dragonfly: bool,
}

/// Where a flying bee moves toward the player this tick, before the floor
/// and ceiling limit its height; `None` if the player is out of range, and
/// it stays where it is.
///
/// Port of the chase in `MoveFlyingBee_Flying`
/// (original/src/Enemies/Enemy_Bee_Flying.c): the bee aims at a point
/// `dist_from_player` from the player, on its own side, and higher the
/// farther it is.
fn chase_position(coord: Vec3, dist_from_player: f32, target: &BeeTarget, dt: f32) -> Option<Vec3> {
    let me = Vec2::new(target.position.x, target.position.z);
    let my_y = target.position.y + target.bottom;
    if quick_distance(coord.xz(), me) >= CHASE_RANGE {
        return None;
    }
    let away = (coord.xz() - me).normalize_or_zero();
    let mut dist = dist_from_player;
    if target.dragonfly {
        dist += DRAGONFLY_EXTRA_DISTANCE;
    }
    let aim = me + away * dist;
    let gap = (aim - coord.xz()).clamp(Vec2::splat(-MAX_CHASE_GAP), Vec2::splat(MAX_CHASE_GAP));
    let xz = coord.xz() + gap * (BEE_ACCEL * dt);

    let dist = (quick_distance(xz, me) - BEE_CLOSEST).max(0.0);
    let min_y = if target.ball { BEE_MINY_BALL } else { BEE_MINY };
    let aim_y = my_y + dist * BEE_HEIGHT_FACTOR + min_y;
    let y = coord.y + (aim_y - coord.y) * BEE_ACCEL * dt;
    Some(Vec3::new(xz.x, y, xz.y))
}

/// Keeps a bee below the ceiling and above the floor (the end of the chase
/// in `MoveFlyingBee_Flying`). Without a ceiling, the map's ceiling is far
/// above everything, as the original skips the check (`gDoCeiling`).
fn limit_height(map: &TerrainMap, mut coord: Vec3) -> Vec3 {
    let ceiling = map.height_at(coord.x, coord.z, LayerKind::Ceiling).0 - CEILING_CLEARANCE;
    coord.y = coord.y.min(ceiling);
    coord.y = coord
        .y
        .max(map.floor_height(coord.x, coord.z) + FLOOR_CLEARANCE);
    coord
}

/// Whether a flying bee at `coord` starts a dive at the player, and how
/// much of its horizontal velocity it keeps if so.
///
/// Port of the attack check in `MoveFlyingBee_Flying`
/// (original/src/Enemies/Enemy_Bee_Flying.c). It measures from the
/// player's origin, not its feet.
fn dive_slowdown(coord: Vec3, target: &BeeTarget) -> Option<f32> {
    let ((low, high), range, slowdown) = if target.dragonfly {
        (
            DIVE_HEIGHT_DRAGONFLY,
            ATTACK_RANGE_DRAGONFLY,
            DIVE_START_SLOWDOWN_DRAGONFLY,
        )
    } else {
        (DIVE_HEIGHT, ATTACK_RANGE, DIVE_START_SLOWDOWN)
    };
    let dy = coord.y - target.position.y;
    (dy > low && dy < high && quick_distance(coord.xz(), target.position.xz()) < range)
        .then_some(slowdown)
}

/// A diving bee's velocity after `dt` seconds: it drops, and speeds up
/// toward the player in x and z.
///
/// Port of the acceleration in `MoveFlyingBee_Diving`
/// (original/src/Enemies/Enemy_Bee_Flying.c).
fn dive_velocity(mut velocity: Vec3, coord: Vec3, target: &BeeTarget, dt: f32) -> Vec3 {
    let (gravity, speed) = if target.dragonfly {
        (DIVE_GRAVITY_DRAGONFLY, DIVE_MOVE_SPEED_DRAGONFLY)
    } else {
        (DIVE_GRAVITY, DIVE_MOVE_SPEED)
    };
    velocity.y -= gravity * dt;
    let step = speed * dt;
    velocity.x += if coord.x < target.position.x {
        step
    } else {
        -step
    };
    velocity.z += if coord.z < target.position.z {
        step
    } else {
        -step
    };
    velocity
}

/// Whether a dive is over: the bee landed on something, went below the
/// player, or came too close to the floor (`MoveFlyingBee_Diving`).
fn dive_ended(coord: Vec3, on_ground: bool, player_y: f32, floor: f32) -> bool {
    on_ground || coord.y < player_y || coord.y - floor < DIVE_MIN_HEIGHT
}

/// Replaces a bee's collision kinds if spiking it changes them
/// (`CType |= CTYPE_SPIKED`, `CType &= ~CTYPE_SPIKED`). The layers are
/// immutable, so they are replaced.
fn set_spiked(commands: &mut Commands, bee: Entity, layers: &CollisionLayers, spiked: bool) {
    let kinds = layers.memberships;
    if kinds.has_all(CollisionKind::Spiked) == spiked {
        return;
    }
    let kinds = if spiked {
        kinds | CollisionKind::Spiked
    } else {
        kinds & !LayerMask::from(CollisionKind::Spiked)
    };
    commands
        .entity(bee)
        .insert(CollisionLayers::new(kinds, LayerMask::NONE));
}

/// Throws the white sparks of a dying bee from `at`. Queued, as the move
/// system's collision reads the particles.
fn spark_explosion(commands: &mut Commands, at: Vec3) {
    commands.queue(move |world: &mut World| {
        world.try_resource_scope(|world, mut groups: Mut<ParticleGroups>| {
            let Some(mut random) = world.get_resource_mut::<GameRandom>() else {
                return;
            };
            let Some(group) = groups.new_group(KILL_SPARK_GROUP) else {
                return;
            };
            for _ in 0..KILL_SPARKS {
                let mut spread = || (random.next_f32() - 0.5) * KILL_SPARK_SPEED;
                let velocity = Vec3::new(spread(), spread(), spread());
                let scale = random.next_f32() + 1.0;
                groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
            }
        });
    });
}

/// Kills a bee: it falls, never collides again, throws sparks and, except
/// on the Flight level, never comes back. Returns whether it was killed;
/// a bee that is dying already is left alone.
///
/// Port of `KillFlyingBee` (original/src/Enemies/Enemy_Bee_Flying.c). The
/// original ignores the delta its callers pass, and so does this: the bee
/// keeps its velocity.
fn kill_flying_bee(
    commands: &mut Commands,
    bee: Entity,
    brain: &mut FlyingBeeBrain,
    animator: Option<Mut<SkeletonAnimator>>,
    shape: CollisionBox,
    at: Vec3,
    level: CurrentLevel,
) -> bool {
    if brain.state.is_dying() {
        return false;
    }
    // Sound: stop EFFECT_BUZZ.
    let mut entity = commands.entity(bee);
    if *level != FLIGHT_LEVEL {
        forget_terrain_item(&mut entity);
    }
    // `BottomOff = 0; CType = 0;`, keeping its solid sides.
    let shape = CollisionBox {
        bottom: 0.0,
        ..shape
    };
    entity.insert(solid_object(
        vec![shape],
        LayerMask::NONE,
        SolidSides::NOT_TOP,
    ));
    brain.state = FlyingBeeState::Falling;
    if let Some(mut animator) = animator {
        animator.morph_to(anim::FALL, FALL_MORPH_RATE);
    }
    spark_explosion(commands, at);
    true
}

/// The bees and their models, for the message handlers that kill them.
#[derive(bevy::ecs::system::SystemParam)]
struct BeeKiller<'w, 's> {
    commands: Commands<'w, 's>,
    level: Res<'w, CurrentLevel>,
    bees: Query<
        'w,
        's,
        (
            &'static mut FlyingBeeBrain,
            &'static EnemyModel,
            &'static CollisionBoxes,
            &'static Transform,
        ),
    >,
    animators: Query<'w, 's, &'static mut SkeletonAnimator, Without<FlyingBeeBrain>>,
}

impl BeeKiller<'_, '_> {
    /// [`kill_flying_bee`] for `entity`, if it is a bee.
    fn kill(&mut self, entity: Entity) -> bool {
        let Ok((mut brain, model, boxes, transform)) = self.bees.get_mut(entity) else {
            return false;
        };
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        kill_flying_bee(
            &mut self.commands,
            entity,
            &mut brain,
            self.animators.get_mut(model.0).ok(),
            shape,
            transform.translation,
            *self.level,
        )
    }
}

/// A bee that stings the player dies.
///
/// Port of the flying bee case in `PlayerHitEnemy`
/// (original/src/Player/MyGuy.c); the player's collision has already hurt
/// the player.
fn sting_flying_bees(mut touches: MessageReader<TouchedEnemy>, mut bees: BeeKiller) {
    for touch in touches.read() {
        if touch.spiked {
            bees.kill(touch.enemy);
        }
    }
}

/// A fast enough ball knocks a bee dead. A bee that stung the ball in the
/// same contact is dead already, as `PlayerHitEnemy` returns before the
/// ball's switch, so the kill does nothing then.
///
/// Port of `BallHitFlyingBee` (original/src/Enemies/Enemy_Bee_Flying.c).
fn ball_hit_flying_bees(mut hits: MessageReader<BallHitEnemy>, mut bees: BeeKiller) {
    for hit in hits.read() {
        if hit.ball_speed > KNOCKDOWN_SPEED && bees.kill(hit.enemy) {
            // Sound: EFFECT_POUND at the bee, pitch kMiddleC+2, volume 2.0.
        }
    }
}

/// The kick kills a bee. The original passes the kick's velocity (700 up
/// and along the kick), which `KillFlyingBee` ignores. Bees aren't
/// kickable (`CTYPE_KICKABLE`), so in the original the kick never reaches
/// one; this answers [`EnemyKicked`] all the same.
///
/// Port of the `SKELETON_TYPE_FLYINGBEE` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_flying_bees(mut kicks: MessageReader<EnemyKicked>, mut bees: BeeKiller) {
    for kick in kicks.read() {
        bees.kill(kick.enemy);
    }
}

/// A bee whose health ran out dies.
///
/// Port of the `ENEMY_KIND_FLYINGBEE` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c).
fn kill_hurt_flying_bees(mut killed: MessageReader<EnemyKilled>, mut bees: BeeKiller) {
    for kill in killed.read() {
        bees.kill(kill.enemy);
    }
}

/// The players, for the bees to pick one.
type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static CollisionBoxes,
        &'static PlayerForm,
    ),
    (With<Player>, Without<FlyingBeeBrain>),
>;

/// The player nearest to `coord`, as a bee sees it.
fn bee_target(coord: Vec3, players: &PlayerQuery) -> Option<BeeTarget> {
    let nearest = nearest_player(coord, players.iter().map(|(t, ..)| t.translation))?;
    players
        .iter()
        .find(|(t, ..)| t.translation == nearest)
        .map(|(transform, boxes, form)| BeeTarget {
            position: transform.translation,
            bottom: boxes.0.first().map_or(0.0, |b| b.bottom),
            ball: *form == PlayerForm::Ball,
            dragonfly: riding_dragonfly(),
        })
}

/// Moves the bees by their state. Leaving the item window
/// (`TrackTerrainItem`) is handled by `DespawnOutOfRange`.
///
/// Port of `MoveFlyingBee`, `MoveFlyingBee_Flying`, `MoveFlyingBee_Diving`,
/// `MoveFlyingBee_Fall`, `MoveFlyingBee_Dead` and `UpdateFlyingBee`
/// (original/src/Enemies/Enemy_Bee_Flying.c). Each bee goes for the
/// nearest player.
///
/// The original sets a flying bee's `gDelta` to how far it moved this
/// frame, which the dive and the fall then use as a per-second velocity;
/// that is kept, as the distance it would move in a 60 fps frame. When a
/// hurt kills a bee in the middle of its collision, its move ends there:
/// the original would still let it start a dive in its fall.
#[allow(clippy::too_many_arguments)]
fn move_flying_bees(
    mut commands: Commands,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    mut collision: EnemyCollision,
    mut bees: Query<(
        EnemyBody,
        &mut FlyingBeeBrain,
        &EnemyModel,
        &CollisionLayers,
    )>,
    mut animators: Query<&mut SkeletonAnimator, Without<FlyingBeeBrain>>,
    players: PlayerQuery,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    if dt <= 0.0 {
        return;
    }
    for (mut body, mut brain, model, layers) in &mut bees {
        let bee = body.entity;
        let shape = body.boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        let start = body.transform.translation;
        let target = bee_target(start, &players);
        let mut animator = animators.get_mut(model.0).ok();

        match brain.state {
            FlyingBeeState::Flying => {
                set_spiked(&mut commands, bee, layers, false);
                let mut coord = start;
                if let Some(target) = &target
                    && let Some(chased) = chase_position(start, brain.dist_from_player, target, dt)
                {
                    coord = limit_height(&map, chased);
                }
                if let Some(target) = &target {
                    let (yaw, _) = turn_toward(
                        yaw_of(body.transform.rotation),
                        coord.xz(),
                        target.position.xz(),
                        FLY_TURN_SPEED * dt,
                    );
                    body.transform.rotation = Quat::from_rotation_y(yaw);
                }
                body.transform.translation = coord;
                **body.velocity = (coord - start) / (dt * ORIGINAL_FRAME_RATE);
            }
            FlyingBeeState::Diving => {
                set_spiked(&mut commands, bee, layers, true);
                let target = target.unwrap_or(BeeTarget {
                    position: start,
                    bottom: 0.0,
                    ball: false,
                    dragonfly: false,
                });
                let velocity = dive_velocity(**body.velocity, start, &target, dt);
                **body.velocity = velocity;
                move_enemy(&mut body.transform.translation, velocity, dt);
            }
            FlyingBeeState::Falling => {
                let mut velocity = **body.velocity;
                velocity.y -= FALL_GRAVITY * dt;
                **body.velocity = velocity;
                // The original moves a falling bee twice per frame: once
                // itself and once in `MoveEnemy`.
                move_enemy(&mut body.transform.translation, velocity, dt);
                move_enemy(&mut body.transform.translation, velocity, dt);
                collision.collide(&mut body, death_enemy_collision_mask(), &mut |_, _| false);
                if body.ground.on_ground {
                    brain.state = FlyingBeeState::Dead;
                    if let Some(animator) = animator.as_mut() {
                        animator.morph_to(anim::DEATH, DEATH_MORPH_RATE);
                    }
                }
                continue;
            }
            FlyingBeeState::Dead => {
                if culling.is_culled(start, **body.radius) {
                    commands.entity(bee).despawn();
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
        }

        // `KillFlyingBee` never deletes the bee, so its effects can wait
        // until the collision ends. Intentional difference (approved): the
        // original goes on with the frame's flying logic after such a kill,
        // so a dead bee could start a dive and become spiked again; here
        // the dead bee's frame ends.
        let mut killed = false;
        collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| {
            killed = true;
            false
        });
        if killed {
            kill_flying_bee(
                &mut commands,
                bee,
                &mut brain,
                animator,
                shape,
                start,
                *level,
            );
            continue;
        }

        let coord = body.transform.translation;
        match brain.state {
            FlyingBeeState::Flying => {
                if let Some(target) = &target
                    && let Some(slowdown) = dive_slowdown(coord, target)
                {
                    brain.state = FlyingBeeState::Diving;
                    if let Some(animator) = animator.as_mut() {
                        animator.morph_to(anim::DIVE, DIVE_MORPH_RATE);
                    }
                    body.velocity.x *= slowdown;
                    body.velocity.z *= slowdown;
                }
            }
            FlyingBeeState::Diving => {
                let player_y = target.map_or(coord.y, |t| t.position.y);
                let floor = map.floor_height(coord.x, coord.z);
                if dive_ended(coord, body.ground.on_ground, player_y, floor) {
                    brain.state = FlyingBeeState::Flying;
                    if let Some(animator) = animator.as_mut() {
                        animator.morph_to(anim::FLY, FLY_MORPH_RATE);
                    }
                    **body.velocity *= DIVE_END_SLOWDOWN;
                }
                let (yaw, _) = turn_toward(
                    yaw_of(body.transform.rotation),
                    body.previous.xz(),
                    coord.xz(),
                    DIVE_TURN_SPEED * dt,
                );
                body.transform.rotation = Quat::from_rotation_y(yaw);
            }
            FlyingBeeState::Falling | FlyingBeeState::Dead => {}
        }
        // Sound: start or update EFFECT_BUZZ at the bee.
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn target_at(position: Vec3) -> BeeTarget {
        BeeTarget {
            position,
            bottom: 0.0,
            ball: false,
            dragonfly: false,
        }
    }

    #[test]
    fn a_far_player_is_not_chased() {
        let target = target_at(Vec3::new(3000.0, 0.0, 0.0));
        assert_eq!(chase_position(Vec3::ZERO, 200.0, &target, DT), None);
    }

    #[test]
    fn a_bee_closes_on_its_spot_beside_and_above_the_player() {
        let target = target_at(Vec3::ZERO);
        // The bee is 1000 out along +X and wants to be 200 out.
        let coord = Vec3::new(1000.0, 0.0, 0.0);
        let next = chase_position(coord, 200.0, &target, DT).unwrap_or_default();
        // The gap is pinned at 300.
        assert!((next.x - (1000.0 - 300.0 * BEE_ACCEL * DT)).abs() < 1e-3);
        assert_eq!(next.z, 0.0);
        assert!(next.y > 0.0);

        // Settled: hovering at its distance, at its height.
        let spot = Vec3::new(
            200.0,
            (200.0 - BEE_CLOSEST) * BEE_HEIGHT_FACTOR + BEE_MINY,
            0.0,
        );
        let next = chase_position(spot, 200.0, &target, DT).unwrap_or_default();
        assert!((next - spot).length() < 1e-3, "{next}");

        // Lower over the ball.
        let ball = BeeTarget {
            ball: true,
            ..target
        };
        let low = chase_position(spot, 200.0, &ball, DT).unwrap_or_default();
        assert!(low.y < spot.y);
    }

    #[test]
    fn a_bee_dives_only_from_above_and_near() {
        let target = target_at(Vec3::ZERO);
        assert_eq!(
            dive_slowdown(Vec3::new(100.0, 300.0, 0.0), &target),
            Some(DIVE_START_SLOWDOWN)
        );
        // Too low, too high, too far.
        assert_eq!(dive_slowdown(Vec3::new(100.0, 150.0, 0.0), &target), None);
        assert_eq!(dive_slowdown(Vec3::new(100.0, 900.0, 0.0), &target), None);
        assert_eq!(dive_slowdown(Vec3::new(500.0, 300.0, 0.0), &target), None);
        // On the dragonfly, it dives from lower down.
        let riding = BeeTarget {
            dragonfly: true,
            ..target
        };
        assert_eq!(
            dive_slowdown(Vec3::new(100.0, 100.0, 0.0), &riding),
            Some(DIVE_START_SLOWDOWN_DRAGONFLY)
        );
    }

    #[test]
    fn a_diving_bee_drops_and_heads_for_the_player() {
        let target = target_at(Vec3::new(100.0, 0.0, -100.0));
        let velocity = dive_velocity(Vec3::ZERO, Vec3::new(0.0, 300.0, 0.0), &target, 0.5);
        assert_eq!(
            velocity,
            Vec3::new(
                DIVE_MOVE_SPEED * 0.5,
                -DIVE_GRAVITY * 0.5,
                -DIVE_MOVE_SPEED * 0.5
            )
        );
        assert!(dive_ended(Vec3::new(0.0, 300.0, 0.0), true, 0.0, 0.0));
        assert!(dive_ended(Vec3::new(0.0, -1.0, 0.0), false, 0.0, -100.0));
        assert!(dive_ended(Vec3::new(0.0, 40.0, 0.0), false, 0.0, 0.0));
        assert!(!dive_ended(Vec3::new(0.0, 300.0, 0.0), false, 0.0, 0.0));
    }

    #[test]
    fn start_heights_follow_the_level_and_the_item() {
        assert_eq!(start_height(LevelType::Hive, 3), HIVE_START_HEIGHT);
        assert_eq!(start_height(LevelType::Forest, 0), DEFAULT_START_HEIGHT);
        assert_eq!(start_height(LevelType::Forest, 3), 300.0);
    }

    /// A diving bee with its model, for the message handlers.
    fn world_with_bee(level: usize) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.insert_resource(CurrentLevel(level));
        world.insert_resource(GameRandom::default());
        world.init_resource::<ParticleGroups>();
        world.init_resource::<Messages<TouchedEnemy>>();
        world.init_resource::<Messages<BallHitEnemy>>();
        let model = world.spawn(SkeletonAnimator::default()).id();
        let bee = world
            .spawn((
                FlyingBeeBrain {
                    state: FlyingBeeState::Diving,
                    dist_from_player: 200.0,
                },
                EnemyModel(model),
                Transform::default(),
                solid_object(
                    vec![COLLISION_BOX],
                    [CollisionKind::Enemy, CollisionKind::Spiked],
                    SolidSides::NOT_TOP,
                ),
            ))
            .id();
        (world, bee, model)
    }

    fn state(world: &World, bee: Entity) -> Option<FlyingBeeState> {
        world.get::<FlyingBeeBrain>(bee).map(|b| b.state)
    }

    #[test]
    fn a_sting_kills_the_bee_and_the_ball_hit_after_it_is_ignored() {
        let (mut world, bee, model) = world_with_bee(HIVE_LEVEL);
        let player = Entity::PLACEHOLDER;
        world.write_message(TouchedEnemy {
            player,
            enemy: bee,
            spiked: true,
        });
        world.write_message(BallHitEnemy {
            player,
            enemy: bee,
            ball_velocity: Vec3::X * 2000.0,
            ball_speed: 2000.0,
        });
        world
            .run_system_once(sting_flying_bees)
            .expect("the system runs");
        assert_eq!(state(&world, bee), Some(FlyingBeeState::Falling));
        let sparks = world
            .resource::<ParticleGroups>()
            .iter()
            .map(|(_, g)| g.particles().len())
            .sum::<usize>();
        assert_eq!(sparks, KILL_SPARKS);

        world
            .run_system_once(ball_hit_flying_bees)
            .expect("the system runs");
        // No second burst of sparks.
        assert_eq!(world.resource::<ParticleGroups>().iter().count(), 1);
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(anim::FALL)
        );
        let layers = world.get::<CollisionLayers>(bee).map(|l| l.memberships);
        assert_eq!(layers, Some(LayerMask::NONE));
        let boxes = world.get::<CollisionBoxes>(bee).map(|b| b.0.clone());
        assert_eq!(
            boxes,
            Some(vec![CollisionBox {
                bottom: 0.0,
                ..COLLISION_BOX
            }])
        );
    }

    #[test]
    fn a_touch_without_the_sting_or_a_slow_ball_does_nothing() {
        let (mut world, bee, _) = world_with_bee(HIVE_LEVEL);
        let player = Entity::PLACEHOLDER;
        world.write_message(TouchedEnemy {
            player,
            enemy: bee,
            spiked: false,
        });
        world.write_message(BallHitEnemy {
            player,
            enemy: bee,
            ball_velocity: Vec3::X * 900.0,
            ball_speed: 900.0,
        });
        world
            .run_system_once(sting_flying_bees)
            .expect("the system runs");
        world
            .run_system_once(ball_hit_flying_bees)
            .expect("the system runs");
        assert_eq!(state(&world, bee), Some(FlyingBeeState::Diving));
    }

    #[test]
    fn a_fast_ball_kills_the_bee() {
        let (mut world, bee, _) = world_with_bee(FLIGHT_LEVEL);
        world.write_message(BallHitEnemy {
            player: Entity::PLACEHOLDER,
            enemy: bee,
            ball_velocity: Vec3::X * 1200.0,
            ball_speed: 1200.0,
        });
        world
            .run_system_once(ball_hit_flying_bees)
            .expect("the system runs");
        assert_eq!(state(&world, bee), Some(FlyingBeeState::Falling));
    }

    #[test]
    fn spiking_replaces_the_layers_only_when_it_changes() {
        let mut world = World::new();
        let bee = world.spawn_empty().id();
        let plain = CollisionLayers::new(CollisionKind::Enemy, LayerMask::NONE);
        world
            .run_system_once(move |mut commands: Commands| {
                set_spiked(&mut commands, bee, &plain, false);
            })
            .expect("the system runs");
        assert!(world.get::<CollisionLayers>(bee).is_none());
        world
            .run_system_once(move |mut commands: Commands| {
                set_spiked(&mut commands, bee, &plain, true);
            })
            .expect("the system runs");
        let kinds = world.get::<CollisionLayers>(bee).map(|l| l.memberships);
        assert_eq!(
            kinds,
            Some(LayerMask::from([
                CollisionKind::Enemy,
                CollisionKind::Spiked
            ]))
        );
    }

    #[test]
    fn the_forest_and_hive_maps_have_flying_bees() {
        use bugdom_formats::rsrc::ResourceFork;
        for file in ["Terrain/Beach.ter.rsrc", "Terrain/BeeHive.ter.rsrc"] {
            let path = bugdom_formats::original_data_dir().join(file);
            let fork = ResourceFork::open(&path).expect("terrain file");
            let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
            assert!(
                terrain.items.iter().any(|i| i.kind == kind::FLYING_BEE),
                "{file}"
            );
        }
    }
}
