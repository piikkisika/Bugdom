//! The dragonfly ride and its fireballs, and the bat (Dragonfly Attack).
//!
//! Port of original/src/Ride/DragonFly.c (item kind 18) and of the bat in
//! original/src/Items/Traps.c.
//!
//! A dragonfly waits on the ground as a trigger. A bug that lands on it
//! mounts it ([`MountRide`]); from then on the dragonfly flies itself in
//! [`PlayerSystems::Ride`], steered by its rider's controls, and breathes
//! fireballs ([`fireball`]). Flying too high calls a [`bat`] that swallows
//! the bug. When the rider leaves ([`LeftRide`]) the dragonfly drops back
//! to the ground and waits again.
//!
//! The original keeps the ridden dragonfly in `gCurrentDragonFly`; here each
//! dragonfly remembers its rider ([`DragonFly::rider`]) and each rider its
//! ride ([`Riding`]).

pub mod bat;
pub mod fireball;

use std::f32::consts::{PI, TAU};

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::ecs::entity::EntityHashSet;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::assets::skeleton::SkeletonAsset;
use crate::collision::{
    BoxMover, CollisionBox, CollisionCandidates, CollisionKind, CollisionSystems, SolidSides,
    Trigger, TriggerHit, resolve_box_collisions, solid_object,
};
use crate::enemies::{BoundingRadius, EnemyModel, apply_friction, per_frame_friction};
use crate::fences::Fences;
use crate::input::{Action, ControlInput};
use crate::items::kind as item;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource,
};
use crate::level::{CurrentLevel, LevelType};
use crate::objects::{ModelSpawner, attach_shadow};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{
    BugState, Dying, LeftRide, MountRide, Player, PlayerForm, PlayerSystems, RideKind, Riding,
    hops_off,
};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::state::{AppState, LevelAssets};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::DRAGONFLY, add_dragonfly)
        .add_systems(
            FixedUpdate,
            (
                // The waiting dragonfly is a trigger the player lands on, so
                // it moves before the player's collision looks at it.
                land_dragonflies
                    .after(ItemSystems::Track)
                    .before(CollisionSystems::Gather)
                    .before(PlayerSystems::Move),
                drive_dragonflies.in_set(PlayerSystems::Ride),
                // `DoTrig_DragonFly` runs during the player's move; the
                // player takes its seat in `PlayerSystems::Hold`.
                mount_dragonflies
                    .after(PlayerSystems::Move)
                    .before(PlayerSystems::Hold),
                drop_dragonflies.after(PlayerSystems::Hold),
            )
                .run_if(in_state(AppState::InGame)),
        )
        .add_plugins((bat::plugin, fireball::plugin));
}

/// `DRAGONFLY_SCALE`
const DRAGONFLY_SCALE: f32 = 2.0;
/// How hard the dragonfly flies forward, in units per second squared
/// (`MAX_THRUST`).
const MAX_THRUST: f32 = 3000.0;
/// Its top speed, in units per second (`DRAGONFLY_MAX_SPEED`).
pub const DRAGONFLY_MAX_SPEED: f32 = 900.0;
/// The fastest it turns, in radians per second
/// (`DRAGONFLY_MAX_TURN_SPEED`).
const MAX_TURN_SPEED: f32 = 165.0 * PI / 180.0;
/// How far it pitches up or down, in radians (`MAX_TILT`).
const MAX_TILT: f32 = 0.6;
/// How far it banks for each unit of the cross product of its motion and
/// its heading, in radians.
const BANK_PER_SLIP: f32 = -0.6;
/// How far its origin stays above the floor, in units
/// (`DRAGONFLY_BOTTOM_OFF`).
const BOTTOM_OFFSET: f32 = 80.0;
/// Its trigger and collision box (`SetObjectCollisionBounds(newObj,
/// DRAGONFLY_TOP_OFF, -40, -140, 140, 140, -140)`).
const DRAGONFLY_BOX: CollisionBox = CollisionBox::new(70.0, -40.0, -140.0, 140.0, 140.0, -140.0);
/// The size of its shadow (`AttachShadowToObject(newObj, 8, 15, false)`).
const SHADOW_SCALE: Vec2 = Vec2::new(8.0, 15.0);
/// Its turn when placed is the item's `params[0]` in sixteenths of a turn.
const AIM_STEP: f32 = TAU / 16.0;
/// How much of its fence radius it keeps from fences
/// (`DoFenceCollision(theNode, .3)`).
const FENCE_RADIUS_SCALE: f32 = 0.3;

/// Steering from the rider's controls, in radians per steering unit, with
/// the movement keys and with the mouse or a thumbstick
/// (`DriveDragonFly`).
const KEY_TURN_PER_UNIT: f32 = 0.0018;
const MOUSE_TURN_PER_UNIT: f32 = 0.003;

/// Gravity on a dragonfly dropping back to the ground, in units per second
/// squared.
const LAND_GRAVITY: f32 = 1400.0;
/// Its horizontal slowing as it drops, per frame at 60 fps
/// (`ApplyFrictionToDeltas(20, ...)`).
const LAND_FRICTION_PER_FRAME: f32 = 20.0;

/// The dragonfly's animations (`DRAGONFLY_ANIM_*`).
const ANIM_FLY: usize = 0;
const ANIM_WAIT: usize = 1;
/// How fast it morphs into flying when mounted, and into waiting once it
/// has landed, per second.
const FLY_MORPH_RATE: f32 = 5.0;
const WAIT_MORPH_RATE: f32 = 3.0;

/// The joint the bug sits on (`DRAGONFLY_JOINT_TAIL`) and its seat there
/// (`MovePlayerBug_RideDragonFly`).
const DRAGONFLY_JOINT_TAIL: usize = 3;
const SEAT: Vec3 = Vec3::new(0.0, 8.0, 55.0);
/// The head joint, and the mouth fireballs come from, in its space
/// (`DragonFlyShootFireball`).
const HEAD_JOINT: usize = 0;
const MOUTH_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -80.0);

/// `LEVEL_NUM_BEACH` and `LEVEL_NUM_FLIGHT`, where flying too high calls the
/// bat.
const LEVEL_BEACH: usize = 3;
const LEVEL_FLIGHT: usize = 4;
/// On the Beach, the bat comes above this height, in units
/// (`MAX_DRAGONFLY_FLIGHT_HEIGHT`).
const BEACH_MAX_HEIGHT: f32 = 5000.0;
/// On Dragonfly Attack, the bat comes this far above the floor, in units
/// (`MAX_DRAGONFLY_FLIGHT_HEIGHT2`).
const FLIGHT_MAX_HEIGHT: f32 = 4000.0;
/// How far above the dragonfly the bat's dive is aimed from, in units
/// (`MakeBat(gCoord.x, gCoord.y + 100.0f, gCoord.z)`).
const BAT_CALL_HEIGHT: f32 = 100.0;

/// What a dragonfly does without a rider (`DRAGONFLY_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DragonFlyMode {
    /// Waiting on the ground.
    #[default]
    Wait,
    /// Dropping back to the ground after its rider left.
    Land,
}

/// A dragonfly.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DragonFly {
    pub mode: DragonFlyMode,
    /// The bug riding it (`gCurrentDragonFly`).
    pub rider: Option<Entity>,
    /// Its pitch, heading and bank (`Rot`), turned x, then y, then z.
    pub rot: Vec3,
    /// The skeleton's entity, under the dragonfly's.
    pub model: Entity,
}

/// How the dragonfly's steering follows the controls, a player's
/// preference (`gGamePrefs.dragonflyControl`). Without it, a player steers
/// [`DragonFlySteering::Normal`].
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DragonFlySteering {
    #[default]
    Normal,
    InvertX,
    InvertY,
    InvertXY,
}

impl DragonFlySteering {
    /// The turn about y and the pitch, after the preference.
    fn apply(self, turn: Vec2) -> Vec2 {
        match self {
            Self::Normal => turn,
            Self::InvertX => Vec2::new(-turn.x, turn.y),
            Self::InvertY => Vec2::new(turn.x, -turn.y),
            Self::InvertXY => -turn,
        }
    }
}

/// The collision kinds of a waiting dragonfly, which bugs land on and aim
/// their jumps at (`AddDragonFly`), and of one dropping back to the ground,
/// which they can land on but don't aim at (`PlayerOffDragonfly`). A ridden
/// dragonfly has none.
fn trigger_kinds(auto_target: bool) -> LayerMask {
    let mut kinds = LayerMask::from([CollisionKind::Trigger, CollisionKind::PlayerTriggerOnly]);
    if auto_target {
        kinds |= LayerMask::from([CollisionKind::AutoTarget, CollisionKind::AutoTargetJump]);
    }
    kinds
}

fn layers(kinds: LayerMask) -> CollisionLayers {
    CollisionLayers::new(kinds, LayerMask::NONE)
}

/// The full turn of a dragonfly (`Q3Matrix4x4_SetRotate_XYZ`).
fn rotation(rot: Vec3) -> Quat {
    Quat::from_euler(EulerRot::ZYX, rot.z, rot.y, rot.x)
}

/// Splits the dragonfly's turn between its root, which only takes the
/// heading so that its box and shadow stay level, and its model, which
/// takes the rest.
fn split_rotation(rot: Vec3) -> (Quat, Quat) {
    let heading = Quat::from_rotation_y(rot.y);
    (heading, heading.inverse() * rotation(rot))
}

/// Port of `AddDragonFly` (original/src/Ride/DragonFly.c). `params[0]` is
/// its heading in sixteenths of a turn. It hovers above the floor, waiting
/// for a bug to land on it.
fn add_dragonfly(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    level_assets: Res<LevelAssets>,
    skeletons: Res<Assets<SkeletonAsset>>,
) -> bool {
    // The original stops the game here.
    let level_type = level.def().level_type;
    if level_type != LevelType::Forest {
        warn!("Dragonfly items don't belong on {level_type:?} levels");
        return false;
    }
    let Some(skeleton) = level_assets.skeleton(SkeletonType::DragonFly) else {
        error!("The level has no dragonfly skeleton");
        return false;
    };
    let radius = skeletons.get(&skeleton).map_or(0.0, |s| s.radius) * DRAGONFLY_SCALE;
    let (x, z) = (spawn.position.x, spawn.position.y);
    let position = Vec3::new(x, map.floor_height(x, z) + BOTTOM_OFFSET, z);
    let rot = Vec3::new(0.0, f32::from(spawn.params[0]) * AIM_STEP, 0.0);
    let (heading, model_turn) = split_rotation(rot);

    let mut animator = SkeletonAnimator::default();
    animator.set_anim(ANIM_WAIT);
    let model = commands
        .spawn((
            Name::new("Dragonfly model"),
            Skeleton(skeleton),
            animator,
            Transform::from_rotation(model_turn).with_scale(Vec3::splat(DRAGONFLY_SCALE)),
            TransformInterpolation,
        ))
        .id();
    let root = commands
        .spawn((
            Name::new("Dragonfly"),
            DragonFly {
                mode: DragonFlyMode::Wait,
                rider: None,
                rot,
                model,
            },
            // The player's seat finds the skeleton through it.
            EnemyModel(model),
            Transform::from_translation(position).with_rotation(heading),
            Visibility::default(),
            TransformInterpolation,
            PreviousPosition(position),
            Velocity::default(),
            CollisionCandidates::default(),
            BoundingRadius(radius),
            solid_object(vec![DRAGONFLY_BOX], trigger_kinds(true), SolidSides::ALL),
            Trigger {
                sides: SolidSides::TOP,
                solid: true,
            },
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .add_child(model)
        .id();
    attach_shadow(&mut commands, &mut models, root, SHADOW_SCALE, false);
    true
}

/// Drops a dragonfly for `dt` seconds and returns whether it has reached
/// the floor, where it stops. The landing part of `MoveDragonFly`.
fn land_step(coord: &mut Vec3, velocity: &mut Vec3, floor: f32, dt: f32) -> bool {
    velocity.y -= LAND_GRAVITY * dt;
    apply_friction(velocity, per_frame_friction(LAND_FRICTION_PER_FRAME), dt);
    *coord += *velocity * dt;
    if coord.y - BOTTOM_OFFSET <= floor {
        coord.y = floor + BOTTOM_OFFSET;
        velocity.y = 0.0;
        return true;
    }
    false
}

/// Drops the dragonflies their riders left back to the ground; once down,
/// they wait for a bug again and are aimed at by its jumps.
///
/// Port of `MoveDragonFly` (original/src/Ride/DragonFly.c). Going out of
/// range is the items' [`DespawnOutOfRange`]; a waiting dragonfly doesn't
/// move.
fn land_dragonflies(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut dragonflies: Query<(Entity, &mut DragonFly, &mut Transform, &mut Velocity)>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    let dt = time.delta_secs();
    for (entity, mut dragonfly, mut transform, mut velocity) in &mut dragonflies {
        // Sound: the dragonfly's EFFECT_HELICOPTER stops
        // (`StopObjectStreamEffect`).
        if dragonfly.rider.is_some() || dragonfly.mode != DragonFlyMode::Land {
            continue;
        }
        let floor = map.floor_height(transform.translation.x, transform.translation.z);
        if !land_step(&mut transform.translation, &mut velocity, floor, dt) {
            continue;
        }
        dragonfly.mode = DragonFlyMode::Wait;
        commands.entity(entity).insert(layers(trigger_kinds(true)));
        if let Ok(mut animator) = animators.get_mut(dragonfly.model) {
            animator.morph_to(ANIM_WAIT, WAIT_MORPH_RATE);
        }
    }
}

/// A bug landed on a waiting dragonfly: it takes the bug on, stops
/// colliding, stops dropping and starts flying. The ball and a killed bug
/// can't ride it.
///
/// Port of `DoTrig_DragonFly` (original/src/Ride/DragonFly.c). The player's
/// half, sitting down to ride, answers [`MountRide`].
fn mount_dragonflies(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut mounts: MessageWriter<MountRide>,
    mut dragonflies: Query<&mut DragonFly>,
    mut animators: Query<&mut SkeletonAnimator>,
    players: Query<(&PlayerForm, Has<Dying>), With<Player>>,
) {
    for hit in hits.read() {
        let Ok(mut dragonfly) = dragonflies.get_mut(hit.trigger) else {
            continue;
        };
        // A killed bug only collides with solid things in the original, so
        // it never reaches a trigger.
        let Ok((form, dying)) = players.get(hit.mover) else {
            continue;
        };
        if *form == PlayerForm::Ball || dying || dragonfly.rider.is_some() {
            continue;
        }
        dragonfly.rider = Some(hit.mover);
        // No collision while ridden (`CType = 0`), and it stays in range
        // with its rider (`MoveCall = nil`, so no `TrackTerrainItem`).
        commands
            .entity(hit.trigger)
            .insert(layers(LayerMask::NONE))
            .remove::<DespawnOutOfRange>();
        if let Ok(mut animator) = animators.get_mut(dragonfly.model) {
            animator.morph_to(ANIM_FLY, FLY_MORPH_RATE);
        }
        mounts.write(MountRide {
            player: hit.mover,
            riding: Riding {
                ride: hit.trigger,
                kind: RideKind::DragonFly,
                joint: DRAGONFLY_JOINT_TAIL,
                seat: SEAT,
            },
        });
    }
}

/// Lets a dragonfly go once its rider has left it: it levels out and drops
/// back to the ground, where a bug can land on it again.
///
/// Port of `PlayerOffDragonfly` (original/src/Ride/DragonFly.c).
fn player_off(dragonfly: &mut DragonFly, entity: &mut EntityCommands) {
    dragonfly.rider = None;
    dragonfly.mode = DragonFlyMode::Land;
    dragonfly.rot.x = 0.0;
    dragonfly.rot.z = 0.0;
    entity.insert((layers(trigger_kinds(false)), DespawnOutOfRange));
}

/// Applies a dragonfly's turn to its root and model.
fn pose(rot: Vec3, root: &mut Transform, model: Option<Mut<Transform>>) {
    let (heading, model_turn) = split_rotation(rot);
    root.rotation = heading;
    if let Some(mut model) = model {
        model.rotation = model_turn;
    }
}

/// Lets the dragonflies go whose riders have left them, whatever took them
/// off. Port of the calls to `PlayerOffDragonfly` in
/// `MovePlayerBug_RideDragonFly` and `SeeIfBatEatsPlayer`.
fn drop_dragonflies(
    mut commands: Commands,
    mut left: MessageReader<LeftRide>,
    mut dragonflies: Query<(&mut DragonFly, &mut Transform)>,
    mut models: Query<&mut Transform, Without<DragonFly>>,
) {
    for left in left.read() {
        let Ok((mut dragonfly, mut transform)) = dragonflies.get_mut(left.ride) else {
            continue;
        };
        if dragonfly.rider != Some(left.player) {
            continue;
        }
        player_off(&mut dragonfly, &mut commands.entity(left.ride));
        pose(
            dragonfly.rot,
            &mut transform,
            models.get_mut(dragonfly.model).ok(),
        );
    }
}

/// The turn the rider's controls give this tick, in radians about y and x,
/// as `DriveDragonFly` reads `GetMouseDelta`: positive turns right and
/// pitches the nose down.
fn steering_turn(input: &ControlInput, steering: DragonFlySteering, dt: f32) -> Vec2 {
    let per_unit = if input.using_key_control() {
        KEY_TURN_PER_UNIT
    } else {
        MOUSE_TURN_PER_UNIT
    };
    let delta = input.steering(dt);
    // Clamped, so a big mouse jump can't spin it round.
    let max_turn = MAX_TURN_SPEED * dt;
    let turn = Vec2::new(delta.x, -delta.y) * per_unit;
    steering.apply(turn.clamp(Vec2::splat(-max_turn), Vec2::splat(max_turn)))
}

/// Turns the dragonfly by `turn` (about y, then pitch) and limits its
/// pitch.
fn steer(rot: &mut Vec3, turn: Vec2) {
    rot.y -= turn.x;
    rot.x = (rot.x - turn.y).clamp(-MAX_TILT, MAX_TILT);
}

/// Pushes the dragonfly forward along its heading and pitch for `dt`
/// seconds, keeping it under its top speed. Returns the thrust.
fn thrust(rot: Vec3, velocity: &mut Vec3, dt: f32) -> Vec3 {
    let forward = rotation(rot) * Vec3::new(0.0, 0.0, -MAX_THRUST);
    *velocity += forward * dt;
    let speed = velocity.length();
    if speed > DRAGONFLY_MAX_SPEED {
        *velocity *= DRAGONFLY_MAX_SPEED / speed;
    }
    forward
}

/// How far the dragonfly banks into a turn: by how much its motion slips
/// sideways from its thrust.
fn bank(velocity: Vec3, forward: Vec3) -> f32 {
    if velocity.length() <= 0.0 {
        return 0.0;
    }
    let motion = velocity.xz().normalize_or_zero();
    let heading = forward.xz().normalize_or_zero();
    motion.perp_dot(heading) * BANK_PER_SLIP
}

/// Whether a dragonfly here has flown high enough to call the bat.
fn too_high(level: usize, coord: Vec3, floor: f32) -> bool {
    match level {
        LEVEL_BEACH => coord.y > BEACH_MAX_HEIGHT,
        LEVEL_FLIGHT => coord.y - floor > FLIGHT_MAX_HEIGHT,
        _ => false,
    }
}

/// A ridden dragonfly's parts.
type RiddenQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut DragonFly,
        &'static mut Transform,
        &'static mut Velocity,
        &'static PreviousPosition,
        &'static CollisionCandidates,
        &'static BoundingRadius,
    ),
    Without<Player>,
>;

/// A rider as its dragonfly sees it.
type RiderQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Riding,
        &'static BugState,
        &'static ControlInput,
        Option<&'static DragonFlySteering>,
    ),
    With<Player>,
>;

/// Flies the ridden dragonflies, steered by their riders: turning, thrust,
/// banking, collision with solid objects, fences and the floor, fireballs
/// on the kick, and the bat when it flies too high. A dragonfly whose rider
/// hops off this tick doesn't fly, as the original checks the jump first.
///
/// Port of `DriveDragonFly`, `DoDragonFlyCollisionDetect` and
/// `UpdateDragonFly` (original/src/Ride/DragonFly.c).
#[allow(clippy::too_many_arguments)]
fn drive_dragonflies(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    fences: Option<Res<Fences>>,
    level: Res<CurrentLevel>,
    mut bats: bat::BatCaller,
    mut dragonflies: RiddenQuery,
    riders: RiderQuery,
    mut models: Query<
        (&mut Transform, Option<&SkeletonRig>),
        (Without<DragonFly>, Without<Player>),
    >,
) {
    let dt = time.delta_secs();
    let mut bat_calls = Vec::new();
    for (entity, mut dragonfly, mut transform, mut velocity, previous, candidates, radius) in
        &mut dragonflies
    {
        let Some(rider) = dragonfly.rider else {
            continue;
        };
        let Ok((riding, state, input, steering)) = riders.get(rider) else {
            // The rider has gone without leaving the ride.
            player_off(&mut dragonfly, &mut commands.entity(entity));
            continue;
        };
        if riding.ride != entity || RideKind::of_state(*state) != Some(RideKind::DragonFly) {
            continue;
        }
        if hops_off(input) {
            continue;
        }

        let mut rot = dragonfly.rot;
        steer(
            &mut rot,
            steering_turn(input, steering.copied().unwrap_or_default(), dt),
        );
        let mut v = **velocity;
        let forward = thrust(rot, &mut v, dt);
        let mut coord = transform.translation + v * dt;

        let floor = map.floor_height(coord.x, coord.z);
        if too_high(**level, coord, floor) {
            bat_calls.push((rider, coord + Vec3::Y * BAT_CALL_HEIGHT));
        }

        rot.z = bank(v, forward);

        // `DoDragonFlyCollisionDetect`.
        let mover = BoxMover {
            entity,
            is_player: false,
            shape: DRAGONFLY_BOX,
            old_coord: **previous,
            platform_velocity: Vec3::ZERO,
        };
        resolve_box_collisions(
            &mover,
            &mut coord,
            &mut v,
            CollisionKind::Misc.into(),
            &candidates.0,
            dt,
            &mut EntityHashSet::default(),
        );
        if let Some(fences) = fences.as_deref() {
            let bottom = coord.y + DRAGONFLY_BOX.bottom;
            fences.collide(
                &map,
                previous.xz(),
                &mut coord,
                &mut v,
                **radius * FENCE_RADIUS_SCALE,
                bottom,
            );
        }
        let floor = map.floor_height(coord.x, coord.z);
        if coord.y - BOTTOM_OFFSET <= floor {
            coord.y = floor + BOTTOM_OFFSET;
            v.y = 0.0;
        }

        if input.just_pressed(Action::Kick)
            && let Ok((model_transform, Some(rig))) = models.get(dragonfly.model)
        {
            // The head as the last frame drew it, where the dragonfly is
            // now (`FindCoordOnJoint`).
            let base = Affine3A::from_rotation_translation(transform.rotation, coord)
                * model_transform.compute_affine();
            if let Some(mouth) = joint_position(rig, HEAD_JOINT, MOUTH_OFFSET, base) {
                fireball::shoot(&mut commands, mouth, v);
            }
        }

        // Sound: EFFECT_HELICOPTER follows the dragonfly (`UpdateDragonFly`).
        dragonfly.rot = rot;
        transform.translation = coord;
        **velocity = v;
        pose(
            rot,
            &mut transform,
            models.get_mut(dragonfly.model).ok().map(|(t, _)| t),
        );
    }
    bats.call(bat_calls);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn steering_turns_and_limits_the_pitch() {
        let mut rot = Vec3::ZERO;
        steer(&mut rot, Vec2::new(0.1, 0.2));
        assert!((rot.y + 0.1).abs() < 1e-6);
        assert!((rot.x + 0.2).abs() < 1e-6);
        for _ in 0..10 {
            steer(&mut rot, Vec2::new(0.0, 0.2));
        }
        assert_eq!(rot.x, -MAX_TILT);
        for _ in 0..20 {
            steer(&mut rot, Vec2::new(0.0, -0.2));
        }
        assert_eq!(rot.x, MAX_TILT);
    }

    #[test]
    fn the_keys_steer_slower_than_the_mouse_and_both_are_clamped() {
        let right = ControlInput::for_tests(&[Action::Right], &[]);
        let turn = steering_turn(&right, DragonFlySteering::Normal, DT);
        // 1600 steering units per second at 0.0018 radians each.
        // The keys' 0.048 radians per tick just reach the turn limit.
        let expected = (1600.0 * DT * KEY_TURN_PER_UNIT).min(MAX_TURN_SPEED * DT);
        assert!((turn.x - expected).abs() < 1e-6);
        assert_eq!(turn.y, 0.0);
        // Forward pitches the nose down.
        let forward = ControlInput::for_tests(&[Action::Forward], &[]);
        assert!(steering_turn(&forward, DragonFlySteering::Normal, DT).y > 0.0);
        let mut mouse = ControlInput::default();
        mouse.mouse_motion = Vec2::new(200.0, 0.0);
        let turn = steering_turn(&mouse, DragonFlySteering::Normal, DT);
        assert!((turn.x - MAX_TURN_SPEED * DT).abs() < 1e-6);
        let inverted = steering_turn(&forward, DragonFlySteering::InvertXY, DT);
        assert!(inverted.y < 0.0);
    }

    #[test]
    fn thrust_flies_it_along_its_heading_up_to_its_top_speed() {
        let mut v = Vec3::ZERO;
        let forward = thrust(Vec3::ZERO, &mut v, DT);
        assert!(forward.abs_diff_eq(Vec3::new(0.0, 0.0, -MAX_THRUST), 1e-3));
        assert!(v.abs_diff_eq(Vec3::new(0.0, 0.0, -MAX_THRUST * DT), 1e-3));
        // Pitched up, it climbs.
        let mut v = Vec3::ZERO;
        thrust(Vec3::new(0.3, 0.0, 0.0), &mut v, DT);
        assert!(v.y > 0.0);
        // Turned a quarter, it flies along -x.
        let mut v = Vec3::ZERO;
        thrust(Vec3::new(0.0, PI / 2.0, 0.0), &mut v, DT);
        assert!(v.x < 0.0 && v.z.abs() < 1e-3);
        let mut v = Vec3::ZERO;
        for _ in 0..120 {
            thrust(Vec3::ZERO, &mut v, DT);
        }
        assert!((v.length() - DRAGONFLY_MAX_SPEED).abs() < 1e-2);
    }

    #[test]
    fn it_banks_into_the_slip_and_levels_when_still() {
        let forward = Vec3::new(0.0, 0.0, -1.0);
        assert_eq!(bank(Vec3::ZERO, forward), 0.0);
        assert_eq!(bank(Vec3::new(0.0, 0.0, -5.0), forward), 0.0);
        let sliding_right = bank(Vec3::new(1.0, 0.0, 0.0), forward);
        assert!((sliding_right.abs() - 0.6).abs() < 1e-6);
        assert_eq!(bank(Vec3::new(-1.0, 0.0, 0.0), forward), -sliding_right);
    }

    #[test]
    fn the_bat_comes_only_when_too_high_on_its_two_levels() {
        let at = |y| Vec3::new(0.0, y, 0.0);
        assert!(too_high(LEVEL_BEACH, at(5001.0), 4500.0));
        assert!(!too_high(LEVEL_BEACH, at(4999.0), 0.0));
        assert!(too_high(LEVEL_FLIGHT, at(5001.0), 1000.0));
        assert!(!too_high(LEVEL_FLIGHT, at(5001.0), 1002.0));
        assert!(!too_high(0, at(90_000.0), 0.0));
    }

    #[test]
    fn a_left_dragonfly_drops_to_the_floor_and_stops() {
        let mut coord = Vec3::new(0.0, 1000.0, 0.0);
        let mut v = Vec3::new(500.0, 0.0, 0.0);
        let mut ticks = 0;
        while !land_step(&mut coord, &mut v, 100.0, DT) {
            ticks += 1;
            assert!(ticks < 600, "it lands");
        }
        assert_eq!(coord.y, 100.0 + BOTTOM_OFFSET);
        assert_eq!(v.y, 0.0);
        // 1200 units per second squared of friction stops it sideways.
        assert_eq!(v.x, 0.0);
    }

    #[test]
    fn the_split_rotation_adds_up_to_the_whole_turn() {
        let rot = Vec3::new(0.3, 1.2, -0.4);
        let (heading, model) = split_rotation(rot);
        assert!((heading * model).abs_diff_eq(rotation(rot), 1e-5));
        assert!(heading.abs_diff_eq(Quat::from_rotation_y(1.2), 1e-6));
    }

    fn mount_world() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<TriggerHit>>();
        world.init_resource::<Messages<MountRide>>();
        world.init_resource::<Messages<LeftRide>>();
        let model = world.spawn(SkeletonAnimator::default()).id();
        let dragonfly = world
            .spawn((
                DragonFly {
                    mode: DragonFlyMode::Wait,
                    rider: None,
                    rot: Vec3::ZERO,
                    model,
                },
                Transform::default(),
                layers(trigger_kinds(true)),
                DespawnOutOfRange,
            ))
            .id();
        (world, dragonfly, model)
    }

    fn land_on(world: &mut World, dragonfly: Entity, player: Entity) -> Vec<MountRide> {
        world.write_message(TriggerHit {
            trigger: dragonfly,
            mover: player,
            sides: SolidSides::BOTTOM,
        });
        world
            .run_system_once(mount_dragonflies)
            .expect("the system runs");
        world.resource_mut::<Messages<TriggerHit>>().clear();
        world
            .resource_mut::<Messages<MountRide>>()
            .drain()
            .collect()
    }

    #[test]
    fn a_bug_mounts_and_the_dragonfly_drops_when_it_leaves() {
        let (mut world, dragonfly, model) = mount_world();
        let bug = world.spawn((Player, PlayerForm::Bug)).id();
        let mounts = land_on(&mut world, dragonfly, bug);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].player, bug);
        assert_eq!(mounts[0].riding.ride, dragonfly);
        assert_eq!(mounts[0].riding.joint, DRAGONFLY_JOINT_TAIL);
        assert_eq!(mounts[0].riding.seat, SEAT);
        assert_eq!(world.get::<DragonFly>(dragonfly).unwrap().rider, Some(bug));
        let kinds = world.get::<CollisionLayers>(dragonfly).unwrap().memberships;
        assert_eq!(kinds, LayerMask::NONE);
        assert!(world.get::<DespawnOutOfRange>(dragonfly).is_none());
        assert_eq!(world.get::<SkeletonAnimator>(model).unwrap().anim, ANIM_FLY);

        // A second bug can't take it.
        let other = world.spawn((Player, PlayerForm::Bug)).id();
        assert!(land_on(&mut world, dragonfly, other).is_empty());

        world.get_mut::<DragonFly>(dragonfly).unwrap().rot = Vec3::new(0.4, 1.0, 0.2);
        world.write_message(LeftRide {
            player: bug,
            ride: dragonfly,
        });
        world
            .run_system_once(drop_dragonflies)
            .expect("the system runs");
        let left = world.get::<DragonFly>(dragonfly).unwrap();
        assert_eq!(left.rider, None);
        assert_eq!(left.mode, DragonFlyMode::Land);
        assert_eq!(left.rot, Vec3::new(0.0, 1.0, 0.0));
        let kinds = world.get::<CollisionLayers>(dragonfly).unwrap().memberships;
        assert_eq!(kinds, trigger_kinds(false));
        assert!(!kinds.has_all(CollisionKind::AutoTarget));
        assert!(world.get::<DespawnOutOfRange>(dragonfly).is_some());
    }

    #[test]
    fn neither_the_ball_nor_a_killed_bug_mounts() {
        let (mut world, dragonfly, _) = mount_world();
        let ball = world.spawn((Player, PlayerForm::Ball)).id();
        let dead = world
            .spawn((Player, PlayerForm::Bug, Dying { timer: 1.0 }))
            .id();
        assert!(land_on(&mut world, dragonfly, ball).is_empty());
        assert!(land_on(&mut world, dragonfly, dead).is_empty());
        assert_eq!(world.get::<DragonFly>(dragonfly).unwrap().rider, None);
        let kinds = world.get::<CollisionLayers>(dragonfly).unwrap().memberships;
        assert_eq!(kinds, trigger_kinds(true));
    }
}
