//! The bug swinging on an Ant Hill root.
//!
//! Port of `PlayerGrabRootSwing`, `MovePlayerBug_RopeSwing` and
//! `PlayerLeaveRootSwing` (original/src/Player/Player_Bug.c), and of the
//! grab in `MoveRootSwing` (original/src/Items/Items2.c), whose root side is
//! in `items/anthill/root_swing.rs`.
//!
//! The original keeps the root the bug holds and the one it last let go of
//! in globals (`gCurrentRope`, `gCurrentRopeJoint`, `gPrevRope`); here they
//! are [`SwingingOn`] and [`PrevRope`] on the player.

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::bug::BugState;
use super::held::{joint_matrix, model_relative_to};
use super::{Dying, PLAYER_BUG_SCALE, Player, PlayerForm, PlayerModel};
use crate::input::{Action, ControlInput};
use crate::items::{GrabRootSwing, RootSwing};
use crate::math::yaw_of;
use crate::physics::{PreviousPosition, Velocity};
use crate::skeleton::SkeletonRig;

/// Where the bug hangs, in the space of the root's joint
/// (`gRopeSwingOffset`).
const ROPE_SWING_OFFSET: Vec3 = Vec3::new(0.0, -100.0, 25.0);
/// How far the turn input turns the swinging bug, in radians per unit of
/// steering (the `.008` in `MovePlayerBug_RopeSwing`).
const ROPE_SWING_TURN: f32 = 0.008;
/// How fast the bug leaves a root across the ground, and how fast it rises,
/// in units per second (`PlayerLeaveRootSwing`).
pub const ROPE_LEAVE_SPEED: f32 = 3000.0;
pub const ROPE_LEAVE_RISE: f32 = 200.0;

/// The root the bug swings on (`gCurrentRope`) and the joint it holds
/// (`gCurrentRopeJoint`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwingingOn {
    pub root: Entity,
    pub joint: usize,
}

/// The root the bug last grabbed (`gPrevRope`): it can't grab that one again
/// until it has stood or walked.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrevRope(pub Entity);

/// The velocity the bug leaves a root with: its last velocity's direction,
/// fast across the ground, with a small rise. Port of
/// `PlayerLeaveRootSwing`.
pub fn rope_leave_velocity(velocity: Vec3) -> Vec3 {
    let direction = velocity.normalize_or_zero();
    Vec3::new(
        direction.x * ROPE_LEAVE_SPEED,
        ROPE_LEAVE_RISE,
        direction.z * ROPE_LEAVE_SPEED,
    )
}

/// Where the swinging bug's model is in the world, given the root's joint
/// matrix (which includes the root's scale) and the root's scale: the
/// offset, turned by the bug's own yaw, at the bug's scale on the joint.
fn swing_model_matrix(joint: Affine3A, root_scale: f32, yaw: f32) -> Affine3A {
    joint
        * Affine3A::from_scale(Vec3::splat(PLAYER_BUG_SCALE / root_scale))
        * Affine3A::from_rotation_y(yaw)
        * Affine3A::from_translation(ROPE_SWING_OFFSET)
}

/// Lets a jumping or falling bug take hold of the root it touched, unless
/// it just let go of that one, and turns it by the root's heading.
///
/// Port of `PlayerGrabRootSwing` and the checks before it in
/// `MoveRootSwing`.
#[allow(clippy::type_complexity)]
pub(super) fn grab_root_swings(
    mut grabs: MessageReader<GrabRootSwing>,
    mut commands: Commands,
    mut players: Query<
        (
            &PlayerForm,
            &mut BugState,
            &mut Transform,
            Option<&PrevRope>,
            Has<Dying>,
        ),
        With<Player>,
    >,
    roots: Query<&Transform, Without<Player>>,
) {
    for grab in grabs.read() {
        let Ok((form, mut state, mut transform, prev, dying)) = players.get_mut(grab.player) else {
            continue;
        };
        if *form != PlayerForm::Bug
            || dying
            || !matches!(*state, BugState::Jump | BugState::Fall)
            || prev.is_some_and(|p| p.0 == grab.root)
        {
            continue;
        }
        let root_yaw = roots.get(grab.root).map_or(0.0, |t| yaw_of(t.rotation));
        transform.rotation = Quat::from_rotation_y(yaw_of(transform.rotation) + root_yaw);
        *state = BugState::RopeSwing;
        commands.entity(grab.player).insert((
            SwingingOn {
                root: grab.root,
                joint: grab.joint,
            },
            PrevRope(grab.root),
        ));
    }
}

/// Swings each bug with its root: the turn input turns it, its position is
/// the hold point on the root's joint, and its velocity how far that moved
/// this tick. A bug that lets go this tick keeps last tick's place and
/// velocity, as the original lets go before placing it; a bug whose root
/// has gone falls.
///
/// Port of `MovePlayerBug_RopeSwing`, before the bug's own move.
#[allow(clippy::type_complexity)]
pub(super) fn swing_on_roots(
    time: Res<Time>,
    mut players: Query<
        (
            &SwingingOn,
            &mut BugState,
            &ControlInput,
            &PlayerModel,
            &PreviousPosition,
            &mut Transform,
            &mut Velocity,
        ),
        With<Player>,
    >,
    roots: Query<&RootSwing>,
    mut transforms: ParamSet<(
        Query<(&Transform, Option<&SkeletonRig>), Without<Player>>,
        Query<&mut Transform, Without<Player>>,
    )>,
) {
    let dt = time.delta_secs();
    for (swing, mut state, input, model, previous, mut transform, mut velocity) in &mut players {
        if *state != BugState::RopeSwing || input.just_pressed(Action::Jump) {
            continue;
        }
        let Ok(root) = roots.get(swing.root) else {
            *state = BugState::Fall;
            continue;
        };
        let Some((joint, scale)) =
            joint_matrix(&transforms.p0(), swing.root, Some(root.model), swing.joint)
        else {
            continue;
        };
        let yaw = yaw_of(transform.rotation) + input.steering(dt).x * ROPE_SWING_TURN;
        transform.rotation = Quat::from_rotation_y(yaw);
        // `FindCoordOnJoint`: the offset on the joint, without the bug's yaw
        // or scale.
        transform.translation = joint.transform_point3(ROPE_SWING_OFFSET);
        if dt > 0.0 {
            **velocity = (transform.translation - **previous) / dt;
        }
        let local = model_relative_to(
            swing_model_matrix(joint, scale, yaw),
            transform.rotation,
            transform.translation,
        );
        if let Ok(mut model_transform) = transforms.p1().get_mut(model.0) {
            *model_transform = local;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn leaving_a_root_flings_the_bug_along_its_swing() {
        let v = rope_leave_velocity(Vec3::new(30.0, -40.0, 0.0));
        // The direction includes the vertical motion, as in the original.
        assert!((v.x - 0.6 * ROPE_LEAVE_SPEED).abs() < 1e-3);
        assert_eq!(v.y, ROPE_LEAVE_RISE);
        assert_eq!(v.z, 0.0);
        assert_eq!(rope_leave_velocity(Vec3::ZERO), Vec3::Y * ROPE_LEAVE_RISE);
    }

    fn grab(world: &mut World, player: Entity, root: Entity) {
        world.write_message(GrabRootSwing {
            player,
            root,
            joint: 4,
        });
        world
            .run_system_once(grab_root_swings)
            .expect("the system runs");
        world.resource_mut::<Messages<GrabRootSwing>>().clear();
    }

    #[test]
    fn a_falling_bug_grabs_a_root_once_and_turns_with_it() {
        let mut world = World::new();
        world.init_resource::<Messages<GrabRootSwing>>();
        let root = world
            .spawn(Transform::from_rotation(Quat::from_rotation_y(0.5)))
            .id();
        let player = world
            .spawn((
                Player,
                PlayerForm::Bug,
                BugState::Fall,
                Transform::from_rotation(Quat::from_rotation_y(0.25)),
            ))
            .id();
        grab(&mut world, player, root);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::RopeSwing));
        assert_eq!(
            world.get::<SwingingOn>(player),
            Some(&SwingingOn { root, joint: 4 })
        );
        let yaw = world.get::<Transform>(player).map(|t| yaw_of(t.rotation));
        assert!(yaw.is_some_and(|y| (y - 0.75).abs() < 1e-5));

        // Let go and falling again, it can't take the same root.
        world.entity_mut(player).insert(BugState::Fall);
        world.entity_mut(player).remove::<SwingingOn>();
        grab(&mut world, player, root);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Fall));

        // A walking bug doesn't grab at all.
        let other = world.spawn(Transform::default()).id();
        world.entity_mut(player).insert(BugState::Walk);
        grab(&mut world, player, other);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Walk));
    }
}
