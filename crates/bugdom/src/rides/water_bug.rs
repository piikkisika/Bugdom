//! The water bug: a boat on the Pond that the bug pays a coin to ride.
//!
//! Port of original/src/Ride/WaterBug.c. Landing on a water bug's back
//! (its top is a player trigger) takes the player on, once it is paid for;
//! a bug that can't pay sees a caption over its head. Ridden, it drives
//! itself forward, steered by the rider's turn input, and sprays water out
//! of its back; once the rider hops off it coasts to a stop, and goes back
//! to its spot once out of sight.
//!
//! The player's side of riding is in `player/ride.rs`. The original keeps
//! the ridden bug in `gCurrentWaterBug`; here the bug keeps its rider.

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use crate::assets::skeleton::SkeletonAsset;
use crate::camera::GameCamera;
use crate::collision::{
    BoxMover, CollisionBox, CollisionCandidates, CollisionKind, SolidSides, Trigger, TriggerHit,
    resolve_box_collisions, solid_object,
};
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture,
};
use crate::enemies::{EnemyCulling, apply_friction};
use crate::fences::Fences;
use crate::input::ControlInput;
use crate::items::{
    DespawnOutOfRange, ITEM_FLAG_USER1, ItemSpawn, RegisterItemKind, TerrainItemSource,
    TerrainItems, kind,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::{GameRandom, yaw_forward};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{
    Inventory, LeftRide, MountRide, Player, PlayerForm, PlayerSystems, RideKind, RideModel, Riding,
    hops_off,
};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::state::{AppState, LevelAssets};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(kind::WATER_BUG, add_water_bug)
        .add_systems(
            FixedUpdate,
            (
                // The ridden bug drives just before its rider moves; one
                // whose rider left this tick heard it from the holds.
                (leave_water_bugs, drive_water_bugs)
                    .chain()
                    .in_set(PlayerSystems::Ride),
                // Waiting and coasting bugs move with the objects, after the
                // player (`PLAYER_SLOT`), and the trigger is answered then.
                (board_water_bugs, move_water_bugs, move_captions)
                    .chain()
                    .after(PlayerSystems::Move)
                    .before(PlayerSystems::Hold),
            )
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            PostUpdate,
            face_captions
                .before(bevy::transform::TransformSystems::Propagate)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `WATERBUG_SCALE`
const WATER_BUG_SCALE: f32 = 1.4;
/// The Pond's water surface (`WATER_Y`).
const WATER_Y: f32 = 0.0;
/// How far the bug's origin is above its keel (`WATER_BUG_FOOT_OFFSET`).
const FOOT_OFFSET: f32 = 90.0;
/// Forward thrust while ridden, in units per second squared (`MAX_THRUST`),
/// and the top speed, in units per second (`WATERBUG_MAX_SPEED`).
const MAX_THRUST: f32 = 2000.0;
const MAX_SPEED: f32 = 1200.0;
/// The fastest it turns, in radians per second (`WATERBUG_MAX_TURN_SPEED`).
const MAX_TURN_SPEED: f32 = 275.0_f32.to_radians();
/// How far a unit of steering turns it, with the keys and with the mouse.
const KEY_TURN: f32 = 0.003;
const MOUSE_TURN: f32 = 0.004;
/// How hard its turning tilts it, per unit of the cross product of its
/// motion and its thrust.
const TILT_PER_CROSS: f32 = -0.8;
/// Past this tilt it plays its leaning animations, in radians.
const LEAN_TILT: f32 = 0.15;
/// How far its nose rises per unit of speed gained in a 60 fps frame, and
/// how fast the nose moves, in radians per second.
const NOSE_PER_SPEED_GAIN: f32 = 0.02;
const NOSE_RATE: f32 = 1.5;
/// Friction while coasting, in units per second squared.
const COAST_FRICTION: f32 = 200.0;
/// How fast a coasting bug levels out, in radians per second.
const LEVEL_OUT_RATE: f32 = 3.0;
/// Gravity on a waiting bug (`DEFAULT_GRAVITY`).
const GRAVITY: f32 = 2000.0;
/// The ridden bug's animation speed.
const RIDE_ANIM_SPEED: f32 = 2.0;
/// The original's frame rate, at which the nose tilt was tuned.
const ORIGINAL_FPS: f32 = 60.0;
const COLLISION_BOX: CollisionBox = CollisionBox::new(45.0, -200.0, -50.0, 50.0, 50.0, -50.0);

/// Where the rider sits: on joint 0, `(0, 30, 40)` in its own scale
/// (`MovePlayerBug_RideWaterBug`).
const SEAT_JOINT: usize = 0;
const SEAT: Vec3 = Vec3::new(0.0, 30.0, 40.0);

/// The spray: how often a drop leaves, in seconds; where from (the bug's
/// back, on joint 0) and how it flies, in units and units per second.
const SPRAY_INTERVAL: f32 = 0.02;
const SPRAY_OFFSET: Vec3 = Vec3::new(0.0, 0.0, 70.0);
const SPRAY_BACK_SPEED: f32 = 300.0;
const SPRAY_SPREAD: f32 = 50.0;
const SPRAY_RISE: f32 = 500.0;
const SPRAY_RISE_SPREAD: f32 = 100.0;
const SPRAY_POSITION_SPREAD: f32 = 80.0;
const SPRAY_HEIGHT: f32 = 50.0;
const SPRAY_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::NONE,
    gravity: 1400.0,
    magnetism: 0.0,
    base_scale: 45.0,
    decay_rate: -1.7,
    fade_rate: 1.0,
    texture: ParticleTexture::Patchy,
};

/// The "pay first" caption: `POND_MObjType_Caption`, over the bug's head
/// (joint 5) for two seconds.
const CAPTION_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 11);
const CAPTION_JOINT: usize = 5;
const CAPTION_OFFSET: Vec3 = Vec3::new(0.0, 60.0, -40.0);
const CAPTION_TIME: f32 = 2.0;

/// The water bug's animations (`WATERBUG_ANIM_*`).
mod anim {
    pub const WAIT: usize = 0;
    pub const LEFT: usize = 1;
    pub const RIGHT: usize = 2;
    pub const CENTER: usize = 3;
    pub const OUT_OF_SERVICE: usize = 4;
}
const LEAN_MORPH_RATE: f32 = 5.0;

/// What a water bug is doing (`WATERBUG_MODE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterBugMode {
    /// At its spot, floating on the water.
    Waiting,
    /// Left by its rider, slowing to a stop.
    Coast,
    /// Ridden by this player.
    Ride(Entity),
}

/// A water bug.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WaterBug {
    pub mode: WaterBugMode,
    /// The skeleton's entity.
    model: Entity,
    /// Its map item, whose flags remember that it is paid for.
    item: u32,
    /// Where it started and which way it faced (`InitCoord`,
    /// `OriginalRot`).
    home: Vec3,
    home_yaw: f32,
    /// Paid for, for good (`IsPaidFor`).
    paid: bool,
    /// Its heading, nose tilt and lean (`Rot.y`, `Rot.x`, `Rot.z`).
    yaw: f32,
    pitch: f32,
    roll: f32,
    /// Its speed across the water (`Speed`).
    speed: f32,
    /// Bounding radius, for culling and fences.
    radius: f32,
    /// Seconds since the last drop of spray (`gWaterSprayRegulator`) and
    /// its spray's particle group (`gWaterBugParticleGroup`).
    spray_timer: f32,
    spray_group: Option<ParticleGroupId>,
    /// The caption over its head (`ChainNode`).
    caption: Option<Entity>,
}

impl WaterBug {
    fn rotation(&self) -> Quat {
        // `STATUS_BIT_ROTXZY`: x, then z, then y.
        Quat::from_rotation_y(self.yaw)
            * Quat::from_rotation_z(self.roll)
            * Quat::from_rotation_x(self.pitch)
    }
}

/// The caption over an unpaid water bug's head.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct Caption {
    bug: Entity,
    /// Seconds left (`SpecialF[0]`).
    timer: f32,
}

/// The water bug's collision kinds: a player trigger on its top, which
/// the auto-aim also goes for until it is first ridden.
fn water_bug_kinds(auto_target: bool) -> LayerMask {
    let mut kinds = LayerMask::from([CollisionKind::Trigger, CollisionKind::PlayerTriggerOnly]);
    if auto_target {
        kinds |= LayerMask::from([CollisionKind::AutoTarget, CollisionKind::AutoTargetJump]);
    }
    kinds
}

/// Port of `AddWaterBug`. `params[0]` is its heading in sixteenths of a
/// turn, counter-clockwise; the item's `ITEM_FLAGS_USER1` says it is paid
/// for.
fn add_water_bug(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    level: Res<CurrentLevel>,
    level_assets: Res<LevelAssets>,
    skeletons: Res<Assets<SkeletonAsset>>,
) -> bool {
    if level.def().level_type != LevelType::Pond {
        // The original stops the game here.
        warn!("Water bug on a level other than the Pond; skipped");
        return false;
    }
    let Some(skeleton) = level_assets.skeleton(SkeletonType::WaterBug) else {
        error!("The level has no water bug skeleton");
        return false;
    };
    let radius = skeletons.get(&skeleton).map_or(0.0, |s| s.radius) * WATER_BUG_SCALE;
    let home = Vec3::new(spawn.position.x, WATER_Y + FOOT_OFFSET, spawn.position.y);
    let yaw = f32::from(spawn.params[0]) * std::f32::consts::TAU / 16.0;
    let mut animator = SkeletonAnimator::default();
    animator.set_anim(anim::WAIT);
    let model = commands
        .spawn((
            Skeleton(skeleton),
            animator,
            Transform::from_scale(Vec3::splat(WATER_BUG_SCALE)),
        ))
        .id();
    let bug = WaterBug {
        mode: WaterBugMode::Waiting,
        model,
        item: spawn.index,
        home,
        home_yaw: yaw,
        paid: spawn.flags & ITEM_FLAG_USER1 != 0,
        yaw,
        pitch: 0.0,
        roll: 0.0,
        speed: 0.0,
        radius,
        spray_timer: 0.0,
        spray_group: None,
        caption: None,
    };
    let root = commands
        .spawn((
            Name::new("Water bug"),
            Transform::from_translation(home).with_rotation(bug.rotation()),
            Visibility::default(),
            bug,
            RideModel(model),
            Velocity::default(),
            PreviousPosition(home),
            CollisionCandidates::default(),
            solid_object(vec![COLLISION_BOX], water_bug_kinds(true), SolidSides::ALL),
            Trigger {
                sides: SolidSides::TOP,
                solid: true,
            },
            // Only a coasting bug follows its item out of range.
            TerrainItemSource(spawn.index),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    commands.entity(model).insert(ChildOf(root));
    true
}

/// Takes the player that lands on a water bug's back for a ride, once it
/// has paid a coin for it (for good); one that can't pay sees the caption.
/// The ball does nothing to it.
///
/// Port of `DoTrig_WaterBug` and `PutCaptionOnBug`.
#[allow(clippy::too_many_arguments)]
fn board_water_bugs(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut mounts: MessageWriter<MountRide>,
    mut models: ModelSpawner,
    mut items: Option<ResMut<TerrainItems>>,
    mut bugs: Query<&mut WaterBug>,
    mut players: Query<(&PlayerForm, &mut Inventory), With<Player>>,
    rigs: Query<(&GlobalTransform, &SkeletonRig)>,
    mut captions: Query<&mut Caption>,
) {
    for hit in hits.read() {
        let Ok(mut bug) = bugs.get_mut(hit.trigger) else {
            continue;
        };
        let Ok((form, mut inventory)) = players.get_mut(hit.mover) else {
            continue;
        };
        if *form == PlayerForm::Ball || matches!(bug.mode, WaterBugMode::Ride(_)) {
            continue;
        }
        if !bug.paid {
            if !inventory.has_enough_money() {
                put_caption_on_bug(
                    &mut commands,
                    &mut models,
                    hit.trigger,
                    &mut bug,
                    &rigs,
                    &mut captions,
                );
                continue;
            }
            inventory.use_money();
            if let Some(items) = items.as_mut() {
                items.set_flags(bug.item, ITEM_FLAG_USER1);
            }
            bug.paid = true;
        }
        mounts.write(MountRide {
            player: hit.mover,
            riding: Riding {
                ride: hit.trigger,
                kind: RideKind::WaterBug,
                joint: SEAT_JOINT,
                seat: SEAT,
            },
        });
        bug.mode = WaterBugMode::Ride(hit.mover);
        bug.spray_timer = 0.0;
        bug.spray_group = None;
        commands
            .entity(hit.trigger)
            .insert(CollisionLayers::new(
                water_bug_kinds(false),
                LayerMask::NONE,
            ))
            .remove::<DespawnOutOfRange>();
    }
}

/// Puts the caption over the bug's head, or keeps the one there longer.
fn put_caption_on_bug(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    entity: Entity,
    bug: &mut WaterBug,
    rigs: &Query<(&GlobalTransform, &SkeletonRig)>,
    captions: &mut Query<&mut Caption>,
) {
    if let Some(mut caption) = bug.caption.and_then(|c| captions.get_mut(c).ok()) {
        caption.timer = CAPTION_TIME;
        return;
    }
    let Some(at) = rigs.get(bug.model).ok().and_then(|(global, rig)| {
        joint_position(rig, CAPTION_JOINT, CAPTION_OFFSET, global.affine())
    }) else {
        return;
    };
    let caption = commands
        .spawn((
            Name::new("Water bug caption"),
            Transform::from_translation(at),
            Visibility::default(),
            Caption {
                bug: entity,
                timer: CAPTION_TIME,
            },
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        commands,
        caption,
        CAPTION_MODEL,
        Shading::Lit,
        Transform::default(),
    );
    bug.caption = Some(caption);
}

/// A bug whose rider has left coasts, out of service, and follows its item
/// out of range again. Port of the hop-off in `MovePlayerBug_RideWaterBug`.
fn leave_water_bugs(
    mut commands: Commands,
    mut left: MessageReader<LeftRide>,
    mut bugs: Query<&mut WaterBug>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    for leave in left.read() {
        let Ok(mut bug) = bugs.get_mut(leave.ride) else {
            continue;
        };
        bug.mode = WaterBugMode::Coast;
        if let Ok(mut animator) = animators.get_mut(bug.model) {
            animator.morph_to(anim::OUT_OF_SERVICE, LEAN_MORPH_RATE);
            animator.speed = 1.0;
        }
        commands.entity(leave.ride).insert(DespawnOutOfRange);
    }
}

/// What a ridden bug's move reads of the world.
#[derive(bevy::ecs::system::SystemParam)]
struct WaterWorld<'w> {
    time: Res<'w, Time>,
    map: Res<'w, TerrainMap>,
    fences: Option<Res<'w, Fences>>,
    groups: ResMut<'w, ParticleGroups>,
    random: ResMut<'w, GameRandom>,
}

/// Drives each ridden water bug from its rider's controls: it always
/// thrusts forward, turns with the turn input, leans into turns and lifts
/// its nose as it speeds up, and sprays water. The rider faces its way and
/// moves with it, so that it hops off with the bug's velocity.
///
/// Port of `DriveWaterBug` and `SprayWater`. The nose's lift goes by how
/// much speed it gained in a 60 fps frame, as the original's per-frame
/// difference does at that rate.
#[allow(clippy::type_complexity)]
fn drive_water_bugs(
    mut world: WaterWorld,
    mut bugs: Query<
        (
            Entity,
            &mut WaterBug,
            &mut Transform,
            &mut Velocity,
            &PreviousPosition,
            &CollisionCandidates,
        ),
        Without<Player>,
    >,
    mut riders: Query<(&ControlInput, &mut Transform, &mut Velocity), With<Player>>,
    mut animators: Query<(&mut SkeletonAnimator, &SkeletonRig, &GlobalTransform)>,
) {
    let dt = world.time.delta_secs();
    for (entity, mut bug, mut transform, mut velocity, previous, candidates) in &mut bugs {
        let WaterBugMode::Ride(rider) = bug.mode else {
            continue;
        };
        let Ok((input, mut rider_transform, mut rider_velocity)) = riders.get_mut(rider) else {
            continue;
        };
        if hops_off(input) {
            continue;
        }

        // Turn.
        let per_unit = if input.using_key_control() {
            KEY_TURN
        } else {
            MOUSE_TURN
        };
        let max_turn = MAX_TURN_SPEED * dt;
        bug.yaw -= (input.steering(dt).x * per_unit).clamp(-max_turn, max_turn);

        // Thrust, capped across the water.
        let forward = yaw_forward(bug.yaw);
        let thrust = forward * MAX_THRUST;
        let mut delta = **velocity;
        delta.x += thrust.x * dt;
        delta.z += thrust.y * dt;
        delta.y = 0.0;
        let old_speed = bug.speed;
        bug.speed = delta.xz().length();
        if bug.speed > MAX_SPEED {
            let scale = MAX_SPEED / bug.speed;
            delta.x *= scale;
            delta.z *= scale;
            bug.speed = MAX_SPEED;
        }
        let mut coord = transform.translation;
        coord.x += delta.x * dt;
        coord.z += delta.z * dt;

        // Lean into turns, and the matching animation.
        bug.roll = if bug.speed > 0.0 {
            delta
                .xz()
                .normalize_or_zero()
                .perp_dot(thrust.normalize_or_zero())
                * TILT_PER_CROSS
        } else {
            0.0
        };
        let lean = if bug.roll > LEAN_TILT {
            anim::LEFT
        } else if bug.roll < -LEAN_TILT {
            anim::RIGHT
        } else {
            anim::CENTER
        };

        // Lift the nose as it speeds up.
        let gain_per_frame = if dt > 0.0 {
            (bug.speed - old_speed) / dt / ORIGINAL_FPS
        } else {
            0.0
        };
        let target = (gain_per_frame * NOSE_PER_SPEED_GAIN).max(0.0);
        bug.pitch = if bug.pitch < target {
            (bug.pitch + NOSE_RATE * dt).min(target)
        } else {
            (bug.pitch - NOSE_RATE * dt).max(target)
        };

        // Run aground: back to where it was.
        if coord.y - FOOT_OFFSET < world.map.floor_height(coord.x, coord.z) {
            coord.x = previous.x;
            coord.z = previous.z;
        }
        collide(
            &bug, entity, &mut coord, &mut delta, **previous, candidates, &world, dt,
        );

        if let Ok((mut animator, rig, global)) = animators.get_mut(bug.model) {
            animator.speed = RIDE_ANIM_SPEED;
            if animator.anim != lean {
                animator.morph_to(lean, LEAN_MORPH_RATE);
            }
            spray_water(&mut bug, delta, rig, global, &mut world, dt);
        }

        transform.translation = coord;
        transform.rotation = bug.rotation();
        **velocity = delta;
        // The rider faces its way and moves with it (`player->Delta`,
        // `player->Rot.y`).
        **rider_velocity = delta;
        rider_transform.rotation = Quat::from_rotation_y(bug.yaw);
        // Sound: EFFECT_BOATENGINE at the bug, started or kept going.
    }
}

/// Bumps into solid objects and fences. Port of
/// `DoWaterBugCollisionDetect`.
#[allow(clippy::too_many_arguments)]
fn collide(
    bug: &WaterBug,
    entity: Entity,
    coord: &mut Vec3,
    delta: &mut Vec3,
    previous: Vec3,
    candidates: &CollisionCandidates,
    world: &WaterWorld,
    dt: f32,
) {
    let mover = BoxMover {
        entity,
        is_player: false,
        shape: COLLISION_BOX,
        old_coord: previous,
        platform_velocity: Vec3::ZERO,
    };
    resolve_box_collisions(
        &mover,
        coord,
        delta,
        CollisionKind::Misc.into(),
        &candidates.0,
        dt,
        &mut EntityHashSet::default(),
    );
    if let Some(fences) = world.fences.as_deref() {
        fences.collide(
            &world.map,
            previous.xz(),
            coord,
            delta,
            bug.radius,
            coord.y + COLLISION_BOX.bottom,
        );
    }
}

/// Sprays a drop of water out of the bug's back every
/// [`SPRAY_INTERVAL`], starting a new group when its group is gone or
/// full. Port of `SprayWater`.
fn spray_water(
    bug: &mut WaterBug,
    delta: Vec3,
    rig: &SkeletonRig,
    global: &GlobalTransform,
    world: &mut WaterWorld,
    dt: f32,
) {
    bug.spray_timer += dt;
    if bug.spray_timer <= SPRAY_INTERVAL {
        return;
    }
    bug.spray_timer = 0.0;
    let Some(back) = joint_position(rig, 0, SPRAY_OFFSET, global.affine()) else {
        return;
    };
    let random = &mut world.random;
    let away = Vec3::new(delta.x, 0.0, delta.z).normalize_or_zero() * -SPRAY_BACK_SPEED;
    let velocity = Vec3::new(
        away.x + (random.next_f32() - 0.5) * SPRAY_SPREAD,
        SPRAY_RISE + random.next_f32() * SPRAY_RISE_SPREAD,
        away.z + (random.next_f32() - 0.5) * SPRAY_SPREAD,
    );
    let position = Vec3::new(
        back.x + (random.next_f32() - 0.5) * SPRAY_POSITION_SPREAD,
        WATER_Y + SPRAY_HEIGHT,
        back.z + (random.next_f32() - 0.5) * SPRAY_POSITION_SPREAD,
    );
    let scale = random.next_f32() + 2.0;
    // A full group makes way for a new one, once.
    for _ in 0..2 {
        let group = match bug.spray_group {
            Some(group) if world.groups.is_valid(group) => group,
            _ => match world.groups.new_group(SPRAY_GROUP) {
                Some(group) => group,
                None => return,
            },
        };
        bug.spray_group = Some(group);
        if !world
            .groups
            .add_particle(group, position, velocity, scale, FULL_ALPHA)
        {
            return;
        }
        bug.spray_group = None;
    }
}

/// Waiting bugs float at the surface; coasting ones slow to a stop and
/// level out, and go home once out of sight.
///
/// Port of the waiting and coasting modes of `MoveWaterBug`.
#[allow(clippy::type_complexity)]
fn move_water_bugs(
    mut commands: Commands,
    world: WaterWorld,
    culling: EnemyCulling,
    mut bugs: Query<(
        Entity,
        &mut WaterBug,
        &mut Transform,
        &mut Velocity,
        &PreviousPosition,
        &CollisionCandidates,
    )>,
) {
    let dt = world.time.delta_secs();
    for (entity, mut bug, mut transform, mut velocity, previous, candidates) in &mut bugs {
        let mut coord = transform.translation;
        let mut delta = **velocity;
        match bug.mode {
            WaterBugMode::Ride(_) => continue,
            WaterBugMode::Waiting => {
                // Sound: stop the bug's engine.
                delta.y -= GRAVITY * dt;
                coord.y += delta.y * dt;
                let floor = world.map.floor_height(coord.x, coord.z);
                if coord.y - FOOT_OFFSET < floor {
                    coord.y = floor + FOOT_OFFSET;
                    delta.y = 0.0;
                }
                if coord.y - FOOT_OFFSET < WATER_Y {
                    coord.y = WATER_Y + FOOT_OFFSET;
                    delta.y = 0.0;
                }
            }
            WaterBugMode::Coast => {
                // Sound: stop the bug's engine.
                // The original's "cheesy" way home: out of sight, it is back
                // at its spot.
                if culling.is_culled(coord, bug.radius) {
                    coord = bug.home;
                    bug.yaw = bug.home_yaw;
                    bug.pitch = 0.0;
                    bug.roll = 0.0;
                    delta = Vec3::ZERO;
                    bug.mode = WaterBugMode::Waiting;
                    commands.entity(entity).remove::<DespawnOutOfRange>();
                } else {
                    apply_friction(&mut delta, COAST_FRICTION, dt);
                    coord.x += delta.x * dt;
                    coord.z += delta.z * dt;
                    if coord.y - FOOT_OFFSET < world.map.floor_height(coord.x, coord.z) {
                        coord.x = previous.x;
                        coord.z = previous.z;
                    }
                    bug.pitch = level_out(bug.pitch, LEVEL_OUT_RATE * dt);
                    bug.roll = level_out(bug.roll, LEVEL_OUT_RATE * dt);
                    collide(
                        &bug, entity, &mut coord, &mut delta, **previous, candidates, &world, dt,
                    );
                }
            }
        }
        transform.translation = coord;
        transform.rotation = bug.rotation();
        **velocity = delta;
    }
}

/// An angle moved toward 0 by at most `step`.
fn level_out(angle: f32, step: f32) -> f32 {
    if angle < 0.0 {
        (angle + step).min(0.0)
    } else {
        (angle - step).max(0.0)
    }
}

/// Counts each caption down and removes it when its time is up, or when
/// its bug has gone. Port of the timer in `MoveCaption`.
fn move_captions(
    mut commands: Commands,
    time: Res<Time>,
    mut bugs: Query<&mut WaterBug>,
    mut captions: Query<(Entity, &mut Caption)>,
) {
    let dt = time.delta_secs();
    for (entity, mut caption) in &mut captions {
        caption.timer -= dt;
        if caption.timer <= 0.0 || !bugs.contains(caption.bug) {
            if let Ok(mut bug) = bugs.get_mut(caption.bug) {
                bug.caption = None;
            }
            commands.entity(entity).despawn();
        }
    }
}

/// Turns each caption to face the camera (`SetLookAtMatrixAndTranslate`
/// in `MoveCaption`): the model's +Z faces it.
fn face_captions(
    cameras: Query<&GlobalTransform, With<GameCamera>>,
    mut captions: Query<&mut Transform, With<Caption>>,
) {
    let Some(camera) = cameras.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    for mut transform in &mut captions {
        let at = transform.translation;
        if at != camera {
            transform.look_at(at + (at - camera), Vec3::Y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coasting_bug_levels_out_from_either_side() {
        assert_eq!(level_out(0.5, 0.1), 0.4);
        assert_eq!(level_out(-0.5, 0.1), -0.4);
        assert_eq!(level_out(0.05, 0.1), 0.0);
        assert_eq!(level_out(-0.05, 0.1), 0.0);
    }

    #[test]
    fn the_bug_leans_into_a_turn() {
        // Moving straight ahead with thrust ahead: no lean.
        let ahead = Vec2::new(0.0, -1.0);
        assert_eq!(ahead.perp_dot(ahead) * TILT_PER_CROSS, 0.0);
        // Moving ahead with thrust turned left: it leans one way, turned
        // right the other.
        let left = Vec2::new(-1.0, -1.0).normalize();
        let right = Vec2::new(1.0, -1.0).normalize();
        let lean_left = ahead.perp_dot(left) * TILT_PER_CROSS;
        let lean_right = ahead.perp_dot(right) * TILT_PER_CROSS;
        assert!(lean_left * lean_right < 0.0);
        assert!(lean_left.abs() > LEAN_TILT);
    }

    #[test]
    fn only_an_unridden_bug_is_aimed_at() {
        assert!(water_bug_kinds(true).has_all(CollisionKind::AutoTarget));
        assert!(!water_bug_kinds(false).has_all(CollisionKind::AutoTarget));
        assert!(water_bug_kinds(false).has_all(CollisionKind::Trigger));
    }
}
