//! The root swings: roots hanging from the Ant Hill's ceiling, swaying in
//! step, which a jumping bug grabs.
//!
//! Port of `InitRootSwings`, `UpdateRootSwings`, `AddRootSwing`,
//! `MoveRootSwing` and `SetRootAnimTimeIndex`
//! (original/src/Items/Items2.c).
//!
//! The player's side, swinging on the root (`PlayerGrabRootSwing`,
//! `MovePlayerBug_RopeSwing`, `PlayerLeaveRootSwing` in
//! original/src/Player/Player_Bug.c), needs a rope-swing state in the
//! bug's controller that doesn't exist yet. Until it does, the root sends
//! [`GrabRootSwing`] and nothing answers it.

use std::f32::consts::TAU;

use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::collision::{CollisionBox, CollisionBoxes};
use crate::items::kind as item;
use crate::items::scenery::on_level;
use crate::items::{DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource};
use crate::level::{CurrentLevel, LevelType};
use crate::player::{BugState, Dying, Player, PlayerForm, PlayerSystems};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonRig, SkeletonSystems, SkeletonType};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{LayerKind, TerrainMap, TerrainSystems};

pub(super) fn plugin(app: &mut App) {
    app.add_message::<GrabRootSwing>()
        .init_resource::<RootSwingClock>()
        .register_item_kind(item::ROOT_SWING, add_root_swing)
        .add_systems(
            OnEnter(AppState::InGame),
            reset_root_swing_clock
                .after(TerrainSystems::Spawn)
                .before(ItemSystems::Window),
        )
        .add_systems(
            FixedUpdate,
            (advance_root_swing_clock, sway_root_swings, grab_root_swings)
                .chain()
                .after(SkeletonSystems::Advance)
                .before(PlayerSystems::Move)
                .run_if(in_state(AppState::InGame)),
        );
}

/// How many groups of roots sway in step (`MAX_ROOT_SYNCS`).
const ROOT_SYNCS: usize = 4;
/// How much of its sway a root does per second.
const SWAY_RATE: f32 = 0.45;
/// The joints of the root's skeleton (`NUM_JOINTS_IN_ROOT`). The first two
/// are the stump, which can't be grabbed, and the last is a dummy.
const ROOT_JOINTS: usize = 7;
const FIRST_GRAB_JOINT: usize = 2;
/// The half sizes of the box round each joint that a bug grabs, in units.
const GRAB_HALF_HEIGHT: f32 = 15.0;
const GRAB_HALF_WIDTH: f32 = 10.0;
/// A root's scale is this plus a step per its `params[2]`.
const ROOT_BASE_SCALE: f32 = 1.4;
const ROOT_SCALE_STEP: f32 = 0.3;
/// Roots turn in eighths of a turn.
const ROOT_TURN_STEP: f32 = TAU / 8.0;

/// How far through its sway each group of roots is, from 0 to 1
/// (`gRootAnimTimeIndex`).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct RootSwingClock(pub [f32; ROOT_SYNCS]);

impl Default for RootSwingClock {
    /// The groups start a quarter of a sway apart (`InitRootSwings`).
    fn default() -> Self {
        Self(std::array::from_fn(|i| i as f32 / ROOT_SYNCS as f32))
    }
}

impl RootSwingClock {
    /// Port of `UpdateRootSwings`.
    fn advance(&mut self, dt: f32) {
        for t in &mut self.0 {
            *t += dt * SWAY_RATE;
            if *t >= 1.0 {
                *t -= 1.0;
            }
        }
    }
}

/// A root swing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootSwing {
    /// Which group it sways with (`RootSync`).
    pub sync: u8,
    /// The skeleton's entity, under the root's.
    pub model: Entity,
}

/// A jumping or falling bug touched a root swing's joint and grabs it.
///
/// Port of the call to `PlayerGrabRootSwing` in `MoveRootSwing`
/// (original/src/Items/Items2.c). Nothing answers it yet: the bug's
/// controller needs a rope-swing state for it. What the player's side must
/// do (original/src/Player/Player_Bug.c):
///
/// - Ignore the grab when `root` is the root the bug last let go of
///   (`gPrevRope`), which it forgets once it stands or walks.
/// - On a grab: remember `root` and `joint` (`gCurrentRope`,
///   `gCurrentRopeJoint`), morph to `PLAYER_ANIM_ROPESWING` (16) at rate
///   10, and add the root's yaw to the bug's.
/// - Each tick (`MovePlayerBug_RopeSwing`): no steering; the jump key lets
///   go (`PlayerLeaveRootSwing`: the velocity's direction times 3000 in x
///   and z, 200 up, morph to falling at 9, then move as falling); the
///   turn input turns the bug; the bug's model is placed at
///   `(0, -100, 25)` in the joint's space, turned by its yaw, at the
///   bug's scale over the root's; its position is that point, and its
///   velocity how far it moved this tick.
/// - While falling just after letting go, it doesn't aim at boppable
///   objects.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrabRootSwing {
    pub player: Entity,
    pub root: Entity,
    /// The joint of the root's skeleton the bug holds.
    pub joint: usize,
}

fn reset_root_swing_clock(mut clock: ResMut<RootSwingClock>) {
    *clock = RootSwingClock::default();
}

/// Port of `AddRootSwing`. `params[0]` is its turn in eighths, `params[1]`
/// the group it sways with and `params[2]` its size. It hangs from the
/// ceiling.
fn add_root_swing(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    level_assets: Res<LevelAssets>,
) -> bool {
    if !on_level(&level, &[LevelType::AntHill], "Root swing") {
        return false;
    }
    let sync = spawn.params[1];
    if usize::from(sync) >= ROOT_SYNCS {
        warn!("Root swing with sync group {sync}, which doesn't exist");
        return false;
    }
    let Some(skeleton) = level_assets.skeleton(SkeletonType::RootSwing) else {
        error!("The level has no root swing skeleton");
        return false;
    };
    let (x, z) = (spawn.position.x, spawn.position.y);
    let (y, _) = map.height_at(x, z, LayerKind::Ceiling);
    let scale = ROOT_BASE_SCALE + f32::from(spawn.params[2]) * ROOT_SCALE_STEP;
    let root = commands
        .spawn((
            Name::new("Root swing"),
            Transform::from_xyz(x, y, z)
                .with_rotation(Quat::from_rotation_y(f32::from(spawn.params[0]) * ROOT_TURN_STEP)),
            Visibility::default(),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    // It doesn't animate on its own: its time is set from its group's.
    let mut animator = SkeletonAnimator::default();
    animator.speed = 0.0;
    let model = commands
        .spawn((
            Skeleton(skeleton),
            animator,
            Transform::from_scale(Vec3::splat(scale)),
            ChildOf(root),
        ))
        .id();
    commands.entity(root).insert(RootSwing { sync, model });
    true
}

/// Port of `UpdateRootSwings`, which only the Ant Hill's levels run.
fn advance_root_swing_clock(
    time: Res<Time>,
    level: Res<CurrentLevel>,
    mut clock: ResMut<RootSwingClock>,
) {
    if level.def().level_type == LevelType::AntHill {
        clock.advance(time.delta_secs());
    }
}

/// Poses each root at its group's point in the sway. Port of
/// `SetRootAnimTimeIndex`.
fn sway_root_swings(
    clock: Res<RootSwingClock>,
    roots: Query<&RootSwing>,
    mut models: Query<(&mut SkeletonAnimator, &SkeletonRig)>,
) {
    for root in &roots {
        let Ok((mut animator, rig)) = models.get_mut(root.model) else {
            continue;
        };
        let Some(anim) = rig.definition.animations.get(animator.anim) else {
            continue;
        };
        let t = clock.0.get(usize::from(root.sync)).copied().unwrap_or(0.0);
        animator.time = max_keyframe_tick(anim) * t;
    }
}

/// The last keyframe's tick in an animation (`CalcMaxKeyFrameTime`).
fn max_keyframe_tick(anim: &bugdom_formats::skeleton::Animation) -> f32 {
    anim.keyframes
        .iter()
        .flatten()
        .map(|keyframe| keyframe.tick)
        .max()
        .unwrap_or(0)
        .max(0) as f32
}

/// Sends [`GrabRootSwing`] for each jumping or falling bug that touches a
/// root's joints. Port of the latching part of `MoveRootSwing`, for every
/// player rather than the one.
///
/// The original goes by the bug's animation (jumping or falling); this
/// goes by its state, which follows it. The invisible object the original
/// keeps on the second-to-last joint (`ChainNode`) has no collision or
/// model, so nothing finds it; it isn't ported.
fn grab_root_swings(
    roots: Query<(Entity, &RootSwing, &Transform)>,
    models: Query<(&SkeletonRig, &Transform)>,
    players: Query<
        (Entity, &PlayerForm, &BugState, &Transform, &CollisionBoxes),
        (With<Player>, Without<Dying>),
    >,
    mut grabs: MessageWriter<GrabRootSwing>,
) {
    let jumpers: Vec<_> = players
        .iter()
        .filter(|(_, form, state, ..)| {
            **form == PlayerForm::Bug && matches!(state, BugState::Jump | BugState::Fall)
        })
        .map(|(entity, _, _, transform, boxes)| {
            let world: Vec<CollisionBox> =
                boxes.0.iter().map(|b| b.at(transform.translation)).collect();
            (entity, world)
        })
        .collect();
    if jumpers.is_empty() {
        return;
    }
    for (entity, root, root_transform) in &roots {
        let Ok((rig, model_transform)) = models.get(root.model) else {
            continue;
        };
        let base = root_transform.compute_affine() * model_transform.compute_affine();
        for (player, boxes) in &jumpers {
            if let Some(joint) = grabbed_joint(rig, base, boxes) {
                grabs.write(GrabRootSwing {
                    player: *player,
                    root: entity,
                    joint,
                });
            }
        }
    }
}

/// The joint a player with these world boxes grabs, if any: the first
/// grabbable joint whose box touches one of them, and the one before the
/// dummy for the dummy.
fn grabbed_joint(rig: &SkeletonRig, base: Affine3A, boxes: &[CollisionBox]) -> Option<usize> {
    (FIRST_GRAB_JOINT..ROOT_JOINTS).find_map(|joint| {
        let p = rig.joint_transform(joint, base)?.translation;
        let grab = CollisionBox::new(
            p.y + GRAB_HALF_HEIGHT,
            p.y - GRAB_HALF_HEIGHT,
            p.x - GRAB_HALF_WIDTH,
            p.x + GRAB_HALF_WIDTH,
            p.z + GRAB_HALF_WIDTH,
            p.z - GRAB_HALF_WIDTH,
        );
        boxes
            .iter()
            .any(|b| grab.overlaps(b))
            .then_some(joint.min(ROOT_JOINTS - 2))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_sway_a_quarter_apart_and_wrap() {
        let mut clock = RootSwingClock::default();
        assert_eq!(clock.0, [0.0, 0.25, 0.5, 0.75]);
        clock.advance(1.0);
        assert!((clock.0[0] - 0.45).abs() < 1e-6);
        assert!((clock.0[3] - 0.2).abs() < 1e-6);
        // A full sway takes 1 / 0.45 seconds.
        let mut clock = RootSwingClock::default();
        for _ in 0..1000 {
            clock.advance(1.0 / 0.45 / 1000.0);
        }
        assert!(clock.0.iter().all(|t| (0.0..1.0).contains(t)));
        assert!(clock.0[1] > 0.249 && clock.0[1] < 0.251);
    }
}
