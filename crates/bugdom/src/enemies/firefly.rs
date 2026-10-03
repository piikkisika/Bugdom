//! The firefly: it circles where it was placed until the player comes
//! near, then swoops down, picks the player up and carries it off to its
//! target, where it lets go and flies away.
//!
//! Port of original/src/Enemies/Enemy_FireFly.c. Fireflies come from map
//! items ([`kind::FIREFLY`], `AddFireFly`) on the Night level. Their
//! targets are map items too ([`kind::FIREFLY_TARGET`]), which spawn
//! nothing (`NilAdd`); a firefly reads them from [`TerrainItems`] when it
//! picks the player up (`FindFireFlyTarget`).
//!
//! The original keeps the one chasing and the one carrying firefly in
//! globals (`gCurrentChasingFireFly`, `gCurrentCarryingFireFly`). Here
//! they follow from the fireflies' states: the chaser is the firefly
//! chasing or carrying, the carrier the one carrying, and the player knows
//! its carrier through [`CarriedBy`]. Fireflies count only toward their
//! own kind, not the total of enemies; the enemy base does that from
//! [`EnemyKind::counts_toward_total`].
//!
//! The glow is a separate billboard that follows the firefly
//! ([`FireFlyGlowOf`]), and the firefly's shadow is the Night level's
//! glowing shadow (`AttachGlowShadowToObject`).

use avian3d::prelude::LayerMask;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bugdom_formats::terrain::Item;

use super::{
    EnemyKilled, EnemyKind, EnemySkeleton, EnemySpawner, EnemySystems, ORIGINAL_FRAME_RATE,
    nearest_player,
};
use crate::camera::GameCamera;
use crate::collision::{
    BoxMover, BoxTarget, CollisionBox, CollisionBoxes, CollisionCandidates, CollisionKind,
    CollisionSystems, SolidSides, resolve_box_collisions,
};
use crate::effects::GlowMaterial;
use crate::items::{ItemSpawn, ItemSystems, RegisterItemKind, TerrainItems, kind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_of};
use crate::objects::{
    ModelFile, ModelRef, ModelSpawner, ObjectMaterial, Shading, Shadow, ShadowOf,
};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{BugState, Hold, HoldPlayer, Player, PlayerForm, PlayerSystems, ReleasePlayer};
use crate::skeleton::SkeletonType;
use crate::state::AppState;
use crate::terrain::{MAP_TO_WORLD, TerrainMap};

pub struct FireFlyPlugin;

impl Plugin for FireFlyPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::FIREFLY, add_firefly)
            .add_systems(
                FixedUpdate,
                (
                    // The original puts fireflies before the player in the
                    // object list ("we need this to be *before* player"),
                    // so the carried bug hangs where its firefly is this
                    // tick, and the firefly sees where the player was last
                    // tick.
                    move_fireflies
                        .after(ItemSystems::Track)
                        .after(CollisionSystems::Gather)
                        .after(PlayerSystems::Kick)
                        .after(EnemySystems::Kicked)
                        .before(PlayerSystems::Move)
                        .run_if(in_state(AppState::InGame)),
                    kill_fireflies.in_set(EnemySystems::Killed),
                ),
            )
            .add_systems(Update, make_parts_glow.run_if(in_state(AppState::InGame)))
            .add_systems(
                PostUpdate,
                place_glows
                    .before(TransformSystems::Propagate)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// `FIREFLY_SCALE`.
const SCALE: f32 = 0.6;
/// The glow's size (`FLARE_SCALE`), and how much bigger it flutters at
/// random each tick.
const FLARE_SCALE: f32 = 2.2;
const FLARE_FLUTTER: f32 = 0.3;
/// How high above the floor a firefly's orbit is centred, in units
/// (`FIREFLY_FLIGHT_HEIGHT`).
const FLIGHT_HEIGHT: f32 = 400.0;
/// How far from that centre a firefly starts, in units, across the whole
/// width in x and z and in y.
const START_SPREAD_XZ: f32 = 400.0;
const START_SPREAD_Y: f32 = 300.0;
/// Its starting horizontal velocity, in units per second, across the
/// whole width.
const START_SPEED_SPREAD: f32 = 500.0;
/// The collision box (`SetObjectCollisionBounds(newObj,100,-100,-100,100,100,-100)`).
const COLLISION_BOX: CollisionBox = CollisionBox::new(100.0, -100.0, -100.0, 100.0, 100.0, -100.0);
/// The glowing shadow's size (`AttachGlowShadowToObject(newObj, 11, 11, true)`).
const SHADOW_SCALE: f32 = 11.0;

/// The orbit's pull toward its centre, in units per second squared, times
/// the distance: it pulls harder the closer the firefly is.
const ORBIT_PULL: f32 = 200_000.0;
/// The distance the pull is measured from at the closest, in units.
const ORBIT_MIN_DISTANCE: f32 = 40.0;
/// How close the player must be, in x and z, for a firefly to chase it
/// (`FIREFLY_CHASE_RANGE`).
const CHASE_RANGE: f32 = 200.0;
/// How strongly a chasing firefly is drawn to its aim, per second, per
/// unit of the gap and per unit of the distance (the `.01` in
/// `FireFlyChasePlayer`).
const CHASE_PULL: f32 = 0.01;
/// How far above the player a chasing firefly aims, in units
/// (`NAB_HEIGHT`).
const NAB_HEIGHT: f32 = 300.0;
/// The nab: the firefly must be above the player and less than this much
/// above [`NAB_HEIGHT`], and this close in x and z, in units.
const NAB_HEIGHT_MARGIN: f32 = 20.0;
const NAB_RANGE: f32 = 40.0;
/// Turn speed, in radians per second (`FIREFLY_TURN_SPEED`), and how much
/// faster it turns while carrying.
const TURN_SPEED: f32 = 4.0;
const CARRY_TURN_FACTOR: f32 = 1.5;

/// A carrying firefly's acceleration and top speed, in units per second
/// (squared).
const CARRY_ACCEL: f32 = 500.0;
const CARRY_MAX_SPEED: f32 = 1000.0;
/// The height above the floor a carrying firefly climbs to, in units
/// (`CARRY_HEIGHT`), and the lowest it goes.
const CARRY_HEIGHT: f32 = 1000.0;
const CARRY_MIN_HEIGHT: f32 = 250.0;
/// How fast it climbs below [`CARRY_HEIGHT`], and sinks above it, in units
/// per second squared.
const CARRY_CLIMB: f32 = 2000.0;
const CARRY_SINK: f32 = 1500.0;
/// Above [`CARRY_HEIGHT`], a rise faster than this (units per second) is
/// halved every frame of the original.
const CARRY_RISE_DAMP_ABOVE: f32 = 1.0;
const CARRY_RISE_DAMP_PER_FRAME: f32 = 0.5;
/// How far below itself a carrying firefly checks for liquid, and how far
/// above the liquid's box it then stays, in units.
const LIQUID_CLEARANCE: f32 = 100.0;
/// How close to the target, in x and z, it lets go, in units.
const DROP_RANGE: f32 = 200.0;
/// How fast a firefly that has let go climbs away, in units per second
/// squared.
const GO_AWAY_CLIMB: f32 = 800.0;

/// The glow (`NIGHT_MObjType_FireFlyGlow`) and the glowing shadow
/// (`NIGHT_MObjType_GlowShadow`).
const GLOW_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 1);
const GLOW_SHADOW_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 11);

/// `FIREFLY_ANIM_FLY`, its only animation.
const ANIM_FLY: usize = 0;

/// What a firefly is doing (`FIREFLY_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum FireFlyState {
    /// Circling its home (`OrbitFireFly`).
    #[default]
    Orbit,
    /// Going for a player (`FireFlyChasePlayer`).
    Chase { player: Entity },
    /// Carrying a player to the target, in world x and z
    /// (`FireFlyCarryPlayer`, `gFireFlyTargetX`/`Z`).
    Carry { player: Entity, target: Vec2 },
    /// Let go, flying off until out of range (`FireFlyGoAway`).
    Done,
}

impl FireFlyState {
    /// Whether this firefly is the one chasing (`gCurrentChasingFireFly`),
    /// which it stays while it carries.
    fn is_chasing(self) -> bool {
        matches!(self, Self::Chase { .. } | Self::Carry { .. })
    }

    /// Whether it carries the player (`gCurrentCarryingFireFly`).
    fn is_carrying(self) -> bool {
        matches!(self, Self::Carry { .. })
    }
}

/// A firefly's own state, on its root entity.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FireFlyBrain {
    pub state: FireFlyState,
    /// Which target it carries the player to (`FireFlyTargetID`, the
    /// item's first parameter).
    pub target_id: u8,
}

/// The glow of a firefly, which follows it facing the camera. It is
/// despawned with the firefly (the original's `ChainNode`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = FireFlyGlows)]
pub struct FireFlyGlowOf(pub Entity);

/// A firefly's glows.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
#[relationship_target(relationship = FireFlyGlowOf, linked_spawn)]
pub struct FireFlyGlows(Vec<Entity>);

/// The glow's size this tick, which flutters at random.
#[derive(Component, Debug, Clone, Copy, PartialEq, Deref, DerefMut)]
pub struct GlowFlutter(pub f32);

/// A model whose parts still have to be made to glow.
#[derive(Component, Debug, Clone, Copy, Default)]
struct GlowParts;

/// Port of `AddFireFly` (original/src/Enemies/Enemy_FireFly.c). Like the
/// original, it doesn't check the enemy counts (`MAX_FIREFLY` is unused),
/// and its health and damage stay zero (`FIREFLY_HEALTH` and
/// `FIREFLY_DAMAGE` are unused too). The original stops the game if a
/// firefly is placed on another level; this refuses to spawn it.
fn add_firefly(
    In(spawn): In<ItemSpawn>,
    mut enemies: EnemySpawner,
    level: Res<CurrentLevel>,
    mut random: ResMut<GameRandom>,
) -> bool {
    if level.def().level_type != LevelType::Night {
        error!("AddFireFly: not on this level");
        return false;
    }
    let def = EnemySkeleton {
        // Only `CTYPE_ENEMY`: it can't be kicked and doesn't block the
        // camera.
        kinds: LayerMask::from(CollisionKind::Enemy),
        ..EnemySkeleton::new(
            EnemyKind::FireFly,
            SkeletonType::FireFly,
            spawn.position,
            SCALE,
        )
    }
    .from_item(spawn.index)
    .foot_offset(-FLIGHT_HEIGHT)
    .collision_box(COLLISION_BOX)
    .solid(SolidSides::TOUCHABLE)
    .anim(ANIM_FLY);
    let Some(firefly) = enemies.spawn(def) else {
        return false;
    };

    // The orbit's centre (`InitCoord`) stays where the spawner put it; the
    // firefly starts off it, already moving.
    let (x, z) = (spawn.position.x, spawn.position.y);
    let home = Vec3::new(x, enemies.map().floor_height(x, z) + FLIGHT_HEIGHT, z);
    let mut spread = |width: f32| (random.next_f32() - 0.5) * width;
    let offset = Vec3::new(
        spread(START_SPREAD_XZ),
        spread(START_SPREAD_Y),
        spread(START_SPREAD_XZ),
    );
    let velocity = Vec3::new(spread(START_SPEED_SPREAD), 0.0, spread(START_SPEED_SPREAD));
    let coord = home + offset;
    enemies.commands().entity(firefly).insert((
        Transform::from_translation(coord),
        PreviousPosition(coord),
        Velocity(velocity),
        FireFlyBrain {
            state: FireFlyState::Orbit,
            target_id: spawn.params[0],
        },
    ));

    enemies
        .commands()
        .run_system_cached_with(spawn_firefly_lights, (firefly, coord));
    true
}

/// Gives a new firefly its glowing shadow and its glow. Port of the end of
/// `AddFireFly` (original/src/Enemies/Enemy_FireFly.c).
fn spawn_firefly_lights(
    In((firefly, coord)): In<(Entity, Vec3)>,
    mut commands: Commands,
    mut models: ModelSpawner,
) {
    // The glowing shadow (`AttachGlowShadowToObject`), which lies on
    // objects too.
    let shadow = commands
        .spawn((
            Name::new("Firefly shadow"),
            Shadow {
                scale: Vec2::splat(SHADOW_SCALE),
                on_objects: true,
            },
            ShadowOf(firefly),
            Transform::from_translation(coord),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    spawn_glowing_model(&mut commands, &mut models, shadow, GLOW_SHADOW_MODEL);

    // The glow (`STATUS_BIT_NULLSHADER | STATUS_BIT_GLOW |
    // STATUS_BIT_NOZWRITE | STATUS_BIT_NOFOG | STATUS_BIT_KEEPBACKFACES`).
    let glow = commands
        .spawn((
            Name::new("Firefly glow"),
            FireFlyGlowOf(firefly),
            GlowFlutter(FLARE_SCALE),
            Transform::from_translation(coord).with_scale(Vec3::splat(FLARE_SCALE)),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    spawn_glowing_model(&mut commands, &mut models, glow, GLOW_MODEL);
}

/// Spawns `model` under `parent`, to be made to glow by
/// [`make_parts_glow`].
fn spawn_glowing_model(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    parent: Entity,
    model: ModelRef,
) {
    if let Some(entity) = models.spawn(
        commands,
        parent,
        model,
        Shading::Unlit,
        Transform::default(),
    ) {
        commands.entity(entity).insert(GlowParts);
    }
}

/// Where the firefly with `target_id` carries the player: the target item
/// with that ID, or else the first target item there is, in world x and z.
/// `None` if the level has no target items.
///
/// Port of `FindFireFlyTarget` (original/src/Enemies/Enemy_FireFly.c).
fn find_target(items: &[Item], target_id: u8) -> Option<Vec2> {
    let targets = || items.iter().filter(|i| i.kind == kind::FIREFLY_TARGET);
    targets()
        .find(|i| i.params[0] == target_id)
        .or_else(|| targets().next())
        .map(|i| Vec2::new(f32::from(i.x), f32::from(i.z)) * MAP_TO_WORLD)
}

/// The velocity change of the orbit for `dt` seconds: a pull toward the
/// centre that grows as the firefly gets closer.
///
/// Port of the orbital dynamics in `OrbitFireFly`
/// (original/src/Enemies/Enemy_FireFly.c).
fn orbit_pull(coord: Vec3, home: Vec3, dt: f32) -> Vec3 {
    let to_home = (home - coord).normalize_or_zero();
    let d = coord.distance(home);
    let inverse = if d != 0.0 {
        1.0 / d.max(ORBIT_MIN_DISTANCE)
    } else {
        // The original's odd fallback, harmless as the direction is zero.
        ORBIT_MIN_DISTANCE
    };
    to_home * (dt * inverse * ORBIT_PULL)
}

/// Moves one axis toward `aim`: the velocity grows by the gap times
/// `pull`, and the firefly stops on `aim` instead of passing it.
///
/// Port of each axis of `FireFlyChasePlayer`
/// (original/src/Enemies/Enemy_FireFly.c).
fn approach(coord: &mut f32, velocity: &mut f32, aim: f32, pull: f32, dt: f32) {
    let below = *coord < aim;
    *velocity += (aim - *coord) * pull;
    *coord += *velocity * dt;
    if (below && *coord > aim) || (!below && *coord < aim) {
        *coord = aim;
        *velocity = 0.0;
    }
}

/// A chasing firefly's move for `dt` seconds toward a point
/// [`NAB_HEIGHT`] above the player.
///
/// Port of the move in `FireFlyChasePlayer`
/// (original/src/Enemies/Enemy_FireFly.c).
fn chase(coord: &mut Vec3, velocity: &mut Vec3, player: Vec3, dt: f32) {
    let pull = coord.distance(player) * dt * CHASE_PULL;
    approach(&mut coord.x, &mut velocity.x, player.x, pull, dt);
    approach(&mut coord.z, &mut velocity.z, player.z, pull, dt);
    approach(
        &mut coord.y,
        &mut velocity.y,
        player.y + NAB_HEIGHT,
        pull,
        dt,
    );
}

/// Whether a chasing firefly at `coord` is where it can pick up the player
/// at `player`: above it, not more than a little above [`NAB_HEIGHT`], and
/// nearly straight above in x and z (`FireFlyChasePlayer`).
fn can_nab(coord: Vec3, player: Vec3) -> bool {
    coord.y < player.y + NAB_HEIGHT + NAB_HEIGHT_MARGIN
        && coord.y > player.y
        && quick_distance(coord.xz(), player.xz()) < NAB_RANGE
}

/// Whether the player is in a state a firefly won't pick it up from: the
/// bug dying, carried already, or knocked over (`PLAYER_ANIM_DEATH`,
/// `PLAYER_ANIM_CARRIED`, `PLAYER_ANIM_FALLONBUTT`). The ball can always
/// be picked up.
fn refuses_nab(form: PlayerForm, state: BugState) -> bool {
    form == PlayerForm::Bug
        && matches!(
            state,
            BugState::Death | BugState::Carried | BugState::KnockedOnButt
        )
}

/// A carrying firefly's velocity for `dt` seconds, flying along its
/// heading `yaw` at `speed` and climbing to [`CARRY_HEIGHT`] above the
/// floor at `floor`; it may also lift `coord` off the floor.
///
/// Port of the move in `FireFlyCarryPlayer`
/// (original/src/Enemies/Enemy_FireFly.c). The original halves a fast rise
/// above the carry height once per frame; that is kept as a rate at
/// [`ORIGINAL_FRAME_RATE`].
fn carry_velocity(
    coord: &mut Vec3,
    velocity: Vec3,
    yaw: f32,
    speed: f32,
    floor: f32,
    dt: f32,
) -> Vec3 {
    let mut velocity = Vec3::new(-yaw.sin() * speed, velocity.y, -yaw.cos() * speed);
    if coord.y < floor + CARRY_HEIGHT {
        if coord.y < floor + CARRY_MIN_HEIGHT {
            coord.y = floor + CARRY_MIN_HEIGHT;
            velocity.y = 0.0;
        } else {
            velocity.y += CARRY_CLIMB * dt;
        }
    } else if velocity.y > CARRY_RISE_DAMP_ABOVE {
        velocity.y *= CARRY_RISE_DAMP_PER_FRAME.powf(dt * ORIGINAL_FRAME_RATE);
    } else {
        velocity.y -= CARRY_SINK * dt;
    }
    velocity
}

/// A carrying firefly's speed this tick: last tick's speed (`Speed`, the
/// length of its velocity as `UpdateEnemy` leaves it), sped up, to a limit.
fn carry_speed(velocity: Vec3, dt: f32) -> f32 {
    (velocity.length() + CARRY_ACCEL * dt).min(CARRY_MAX_SPEED)
}

/// The top of the first liquid's box that `point` is in, among the
/// candidates (`DoSimplePointCollision(&p, CTYPE_LIQUID)`).
fn liquid_top(candidates: &[BoxTarget], point: Vec3) -> Option<f32> {
    candidates
        .iter()
        .filter(|t| t.kinds.has_all(CollisionKind::Liquid))
        .find(|t| t.boxes.iter().any(|b| b.contains(point)))
        .and_then(|t| t.boxes.first())
        .map(|b| b.top)
}

/// The players, for the fireflies.
type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static PlayerForm,
        &'static BugState,
    ),
    (With<Player>, Without<FireFlyBrain>),
>;

/// Moves the fireflies by their state, and flutters their glows. Leaving
/// the item window (`TrackTerrainItem`) is handled by `DespawnOutOfRange`,
/// which ends a chase or a carry with the firefly.
///
/// Port of `MoveFireFly`, `OrbitFireFly`, `FireFlyChasePlayer`,
/// `FireFlyCarryPlayer`, `FireFlyGoAway` and `UpdateFireFly`
/// (original/src/Enemies/Enemy_FireFly.c). Each firefly goes for the
/// nearest player, and once it chases one it keeps to that player. The
/// object collisions are `HandleCollisions(theNode, CTYPE_MISC)`: boxes
/// only, without the terrain.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn move_fireflies(
    time: Res<Time>,
    map: Res<TerrainMap>,
    items: Option<Res<TerrainItems>>,
    mut random: ResMut<GameRandom>,
    mut fireflies: Query<
        (
            Entity,
            &mut Transform,
            &mut Velocity,
            &PreviousPosition,
            &CollisionBoxes,
            &CollisionCandidates,
            &super::HomePosition,
            &mut FireFlyBrain,
            Option<&FireFlyGlows>,
        ),
        Without<Player>,
    >,
    mut flutters: Query<&mut GlowFlutter>,
    players: PlayerQuery,
    mut holds: MessageWriter<HoldPlayer>,
    mut releases: MessageWriter<ReleasePlayer>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut chaser = fireflies.iter().any(|(.., b, _)| b.state.is_chasing());
    let mut carrier = fireflies.iter().any(|(.., b, _)| b.state.is_carrying());
    for (
        firefly,
        mut transform,
        mut velocity,
        previous,
        boxes,
        candidates,
        home,
        mut brain,
        glows,
    ) in &mut fireflies
    {
        let mut coord = transform.translation;
        let mut delta = **velocity;
        let mut yaw = yaw_of(transform.rotation);
        let nearest = nearest_player(coord, players.iter().map(|(_, t, ..)| t.translation));
        let collide = |coord: &mut Vec3, delta: &mut Vec3| {
            let mover = BoxMover {
                entity: firefly,
                is_player: false,
                shape: boxes.0.first().copied().unwrap_or(COLLISION_BOX),
                old_coord: **previous,
                platform_velocity: Vec3::ZERO,
            };
            resolve_box_collisions(
                &mover,
                coord,
                delta,
                LayerMask::from(CollisionKind::Misc),
                &candidates.0,
                dt,
                &mut EntityHashSet::default(),
            );
        };

        match brain.state {
            FireFlyState::Orbit => {
                if let Some(player) = nearest {
                    yaw = turn_toward(yaw, coord.xz(), player.xz(), TURN_SPEED * dt).0;
                }
                delta += orbit_pull(coord, **home, dt);
                coord += delta * dt;
                collide(&mut coord, &mut delta);

                // See if close enough to attack.
                if !chaser
                    && let Some((player, ..)) = players.iter().find(|(_, t, ..)| {
                        Some(t.translation) == nearest
                            && quick_distance(coord.xz(), t.translation.xz()) < CHASE_RANGE
                    })
                {
                    brain.state = FireFlyState::Chase { player };
                    chaser = true;
                }
            }
            FireFlyState::Chase { player } => {
                if let Ok((_, player_transform, form, state)) = players.get(player) {
                    let me = player_transform.translation;
                    yaw = turn_toward(yaw, coord.xz(), me.xz(), TURN_SPEED * dt).0;
                    chase(&mut coord, &mut delta, me, dt);
                    collide(&mut coord, &mut delta);

                    if !refuses_nab(*form, *state) && !carrier && can_nab(coord, me) {
                        let target = items
                            .as_deref()
                            .and_then(|items| find_target(&items.items, brain.target_id));
                        let target = target.unwrap_or_else(|| {
                            // The original stops the game here.
                            error!("FindFireFlyTarget: no targets found");
                            coord.xz()
                        });
                        brain.state = FireFlyState::Carry { player, target };
                        carrier = true;
                        holds.write(HoldPlayer {
                            player,
                            hold: Hold::Carried { by: firefly },
                        });
                    }
                } else {
                    collide(&mut coord, &mut delta);
                }
            }
            FireFlyState::Carry { player, target } => {
                // See if it should drop the player off now.
                if quick_distance(coord.xz(), target) < DROP_RANGE {
                    brain.state = FireFlyState::Done;
                    chaser = false;
                    carrier = false;
                    // The bug falls on its next move.
                    releases.write(ReleasePlayer {
                        player,
                        restore_collision: false,
                    });
                }
                yaw = turn_toward(yaw, coord.xz(), target, TURN_SPEED * CARRY_TURN_FACTOR * dt).0;
                let speed = carry_speed(delta, dt);
                let floor = map.floor_height(coord.x, coord.z);
                delta = carry_velocity(&mut coord, delta, yaw, speed, floor, dt);
                coord += delta * dt;

                // Keep above liquid.
                let below = coord - Vec3::Y * LIQUID_CLEARANCE;
                if let Some(top) = liquid_top(&candidates.0, below) {
                    coord.y = top + LIQUID_CLEARANCE;
                }
            }
            FireFlyState::Done => {
                delta.y += GO_AWAY_CLIMB * dt;
                coord += delta * dt;
            }
        }

        transform.translation = coord;
        transform.rotation = Quat::from_rotation_y(yaw);
        **velocity = delta;

        // `UpdateFireFly`: the glow's random flutter.
        for &glow in glows.into_iter().flat_map(|g| g.0.iter()) {
            if let Ok(mut flutter) = flutters.get_mut(glow) {
                **flutter = FLARE_SCALE + random.next_f32() * FLARE_FLUTTER;
            }
        }
        // Sound: start or update EFFECT_BUZZ at the firefly, pitch
        // kMiddleC+3, volume 0.3.
    }
}

/// A firefly whose health ran out vanishes, letting go of the player it
/// carries. Its item can bring it back.
///
/// Port of `KillFireFly` (original/src/Enemies/Enemy_FireFly.c), the
/// `ENEMY_KIND_FIREFLY` case of `KillEnemy`.
fn kill_fireflies(
    mut commands: Commands,
    mut killed: MessageReader<EnemyKilled>,
    fireflies: Query<&FireFlyBrain>,
    mut releases: MessageWriter<ReleasePlayer>,
) {
    for kill in killed.read() {
        let Ok(brain) = fireflies.get(kill.enemy) else {
            continue;
        };
        if let FireFlyState::Carry { player, .. } = brain.state {
            releases.write(ReleasePlayer {
                player,
                restore_collision: false,
            });
        }
        commands.entity(kill.enemy).try_despawn();
    }
}

/// Puts each glow on its firefly, facing the camera, at its fluttering
/// size. It runs every frame, after the firefly's interpolated position is
/// known.
///
/// Port of the glow's update in `UpdateFireFly`
/// (original/src/Enemies/Enemy_FireFly.c), which builds a look-at matrix
/// toward the camera (`SetLookAtMatrixAndTranslate`).
fn place_glows(
    cameras: Query<&GlobalTransform, With<GameCamera>>,
    fireflies: Query<&Transform, (With<FireFlyBrain>, Without<FireFlyGlowOf>)>,
    mut glows: Query<(&FireFlyGlowOf, &GlowFlutter, &mut Transform)>,
) {
    let camera = cameras.iter().next().map(GlobalTransform::translation);
    for (of, flutter, mut transform) in &mut glows {
        let Ok(firefly) = fireflies.get(of.0) else {
            continue;
        };
        let at = firefly.translation;
        let mut placed = Transform::from_translation(at).with_scale(Vec3::splat(**flutter));
        if let Some(camera) = camera
            && camera != at
        {
            // The model's +Z faces the camera.
            placed.look_at(at + (at - camera), Vec3::Y);
        }
        transform.set_if_neq(placed);
    }
}

/// Gives the parts of the glow and the glowing shadow the glow material:
/// unlit, added to what is behind, not writing depth, without fog and
/// two-sided.
fn make_parts_glow(
    mut commands: Commands,
    models: Query<(Entity, &Children), With<GlowParts>>,
    parts: Query<&MeshMaterial3d<ObjectMaterial>>,
    object_materials: Res<Assets<ObjectMaterial>>,
    mut glow_materials: ResMut<Assets<GlowMaterial>>,
) {
    for (model, children) in &models {
        for &part in children {
            let Ok(material) = parts.get(part) else {
                continue;
            };
            let Some(source) = object_materials.get(&material.0) else {
                continue;
            };
            let glow = glow_materials.add(GlowMaterial {
                color: source.base.base_color.to_linear(),
                texture: source.base.base_color_texture.clone(),
                draw_order: 0.0,
            });
            commands
                .entity(part)
                .remove::<MeshMaterial3d<ObjectMaterial>>()
                .insert(MeshMaterial3d(glow));
        }
        commands.entity(model).remove::<GlowParts>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: u16, x: u16, z: u16, id: u8) -> Item {
        Item {
            x,
            z,
            kind,
            params: [id, 0, 0, 0],
            flags: 0,
        }
    }

    #[test]
    fn the_target_with_the_id_wins_then_any_target() {
        let items = [
            item(kind::FIREFLY, 1, 1, 2),
            item(kind::FIREFLY_TARGET, 10, 20, 1),
            item(kind::FIREFLY_TARGET, 30, 40, 2),
        ];
        assert_eq!(
            find_target(&items, 2),
            Some(Vec2::new(30.0, 40.0) * MAP_TO_WORLD)
        );
        assert_eq!(
            find_target(&items, 7),
            Some(Vec2::new(10.0, 20.0) * MAP_TO_WORLD)
        );
        assert_eq!(find_target(&items[..1], 2), None);
    }

    #[test]
    fn the_orbit_pulls_harder_closer_in() {
        let home = Vec3::new(0.0, 400.0, 0.0);
        let far = orbit_pull(home + Vec3::X * 400.0, home, 1.0);
        let near = orbit_pull(home + Vec3::X * 100.0, home, 1.0);
        assert!(far.x < 0.0 && near.x < far.x);
        assert!((far.x + ORBIT_PULL / 400.0).abs() < 1e-2);
        // Inside the closest distance, the pull stops growing.
        let inside = orbit_pull(home + Vec3::X * 10.0, home, 1.0);
        assert!((inside.x + ORBIT_PULL / ORBIT_MIN_DISTANCE).abs() < 1e-2);
        assert_eq!(orbit_pull(home, home, 1.0), Vec3::ZERO);
    }

    #[test]
    fn the_chase_stops_on_its_aim_instead_of_passing_it() {
        let player = Vec3::new(100.0, 0.0, 0.0);
        let mut coord = Vec3::new(90.0, 290.0, 0.0);
        let mut velocity = Vec3::new(5000.0, 0.0, 0.0);
        chase(&mut coord, &mut velocity, player, 1.0 / 60.0);
        assert_eq!(coord.x, 100.0);
        assert_eq!(velocity.x, 0.0);
        // It climbs toward the nab height, short of it.
        assert!(coord.y > 290.0 && coord.y <= NAB_HEIGHT);
    }

    #[test]
    fn the_nab_needs_the_firefly_just_above_the_player() {
        let player = Vec3::new(0.0, 100.0, 0.0);
        assert!(can_nab(player + Vec3::new(10.0, NAB_HEIGHT, 10.0), player));
        assert!(!can_nab(
            player + Vec3::new(0.0, NAB_HEIGHT + 30.0, 0.0),
            player
        ));
        assert!(!can_nab(player - Vec3::Y, player));
        assert!(!can_nab(player + Vec3::new(50.0, 100.0, 0.0), player));
        assert!(refuses_nab(PlayerForm::Bug, BugState::KnockedOnButt));
        assert!(!refuses_nab(PlayerForm::Bug, BugState::Walk));
        assert!(!refuses_nab(PlayerForm::Ball, BugState::Carried));
    }

    #[test]
    fn carrying_climbs_to_its_height_and_speeds_up_to_a_limit() {
        let dt = 1.0 / 60.0;
        assert!(
            (carry_speed(Vec3::new(300.0, 400.0, 0.0), dt) - (500.0 + CARRY_ACCEL * dt)).abs()
                < 1e-3
        );
        assert_eq!(carry_speed(Vec3::X * 2000.0, dt), CARRY_MAX_SPEED);

        // Scraping the floor: lifted to the lowest height, rise stopped.
        let mut coord = Vec3::new(0.0, 100.0, 0.0);
        let v = carry_velocity(&mut coord, Vec3::Y * -50.0, 0.0, 800.0, 0.0, dt);
        assert_eq!(coord.y, CARRY_MIN_HEIGHT);
        assert_eq!(v, Vec3::new(0.0, 0.0, -800.0));
        // Below the carry height it climbs.
        let mut coord = Vec3::new(0.0, 500.0, 0.0);
        let v = carry_velocity(&mut coord, Vec3::ZERO, 0.0, 800.0, 0.0, dt);
        assert!((v.y - CARRY_CLIMB * dt).abs() < 1e-3);
        // Above it a fast rise halves each original frame, a slow one sinks.
        let mut coord = Vec3::new(0.0, 1200.0, 0.0);
        let v = carry_velocity(&mut coord, Vec3::Y * 100.0, 0.0, 800.0, 0.0, dt);
        assert!((v.y - 50.0).abs() < 1e-3);
        let v = carry_velocity(&mut coord, Vec3::ZERO, 0.0, 800.0, 0.0, dt);
        assert!((v.y + CARRY_SINK * dt).abs() < 1e-3);
    }

    #[test]
    fn one_firefly_chases_and_carries_at_a_time() {
        let player = Entity::from_raw_u32(1).expect("a valid index");
        assert!(!FireFlyState::Orbit.is_chasing());
        assert!(FireFlyState::Chase { player }.is_chasing());
        let carry = FireFlyState::Carry {
            player,
            target: Vec2::ZERO,
        };
        assert!(carry.is_chasing() && carry.is_carrying());
        assert!(!FireFlyState::Done.is_chasing());
    }
}
