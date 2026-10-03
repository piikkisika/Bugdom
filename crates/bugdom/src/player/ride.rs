//! The bug riding the water bug or the dragonfly.
//!
//! Port of the player's half of the rides: `MovePlayerBug_RideWaterBug`
//! and `MovePlayerBug_RideDragonFly` (original/src/Player/Player_Bug.c),
//! and the player's side of `DoTrig_WaterBug` (original/src/Ride/WaterBug.c)
//! and `DoTrig_DragonFly` and `PlayerOffDragonfly`
//! (original/src/Ride/DragonFly.c).
//!
//! The original's player move calls into the ride (`DriveWaterBug`,
//! `DriveDragonFly`) and keeps who it rides in globals
//! (`gCurrentWaterBug`, `gCurrentDragonFly`). Here the ride's own plugin
//! drives it in [`PlayerSystems::Ride`](super::PlayerSystems::Ride), just
//! before the player moves, reading its rider's controls; the ride's
//! trigger sends [`MountRide`], and the player keeps [`Riding`] until it
//! leaves the ride's state, when [`LeftRide`] tells the ride.

use avian3d::prelude::LayerMask;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::bug::BugState;
use super::held::{joint_matrix, split_eaten_matrix};
use super::{Dying, PLAYER_BUG_SCALE, Player, PlayerForm, PlayerModel};
use crate::collision::CollisionKind;
use crate::enemies::EnemyModel;
use crate::input::{Action, ControlInput};
use crate::objects::HideShadow;
use crate::skeleton::SkeletonRig;

/// What the player rides. Each has its own way of sitting
/// (`PLAYER_ANIM_RIDEWATERBUG`, `PLAYER_ANIM_RIDEDRAGONFLY`) and of
/// getting off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RideKind {
    WaterBug,
    DragonFly,
}

impl RideKind {
    /// The bug's state while it rides this.
    pub fn state(self) -> BugState {
        match self {
            Self::WaterBug => BugState::RideWaterBug,
            Self::DragonFly => BugState::RideDragonFly,
        }
    }

    /// The ride a bug state rides, if it is a riding state.
    pub fn of_state(state: BugState) -> Option<Self> {
        match state {
            BugState::RideWaterBug => Some(Self::WaterBug),
            BugState::RideDragonFly => Some(Self::DragonFly),
            _ => None,
        }
    }
}

/// The player rides `ride` (`gCurrentWaterBug`, `gCurrentDragonFly`),
/// sitting at `seat` in the space of the ride's `joint`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Riding {
    pub ride: Entity,
    pub kind: RideKind,
    /// The joint of the ride's skeleton the bug sits on (0 for the water
    /// bug, `DRAGONFLY_JOINT_TAIL`).
    pub joint: usize,
    /// The seat in that joint's space, in the bug's units: the original
    /// applies it after the bug's own scale.
    pub seat: Vec3,
}

/// The child entity that carries a ride's skeleton and scale, for a ride
/// that isn't an enemy (an enemy's is its [`EnemyModel`]).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideModel(pub Entity);

/// A ride's trigger takes the player on. Sent by the ride; ignored for the
/// ball and for a killed player, as the original's triggers ignore the
/// ball (`gPlayerMode == PLAYER_MODE_BALL`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct MountRide {
    pub player: Entity,
    pub riding: Riding,
}

/// The player has left its ride: it hopped off, was thrown off, was
/// killed or was taken by an enemy. The ride goes back to its own
/// behaviour (`PlayerOffDragonfly`, `WATERBUG_MODE_COAST`).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeftRide {
    pub player: Entity,
    pub ride: Entity,
}

/// Whether a rider's controls hop it off this tick (`kKey_Jump` in
/// `MovePlayerBug_RideWaterBug` and `MovePlayerBug_RideDragonFly`). The
/// original checks this before it drives the ride, so a ride doesn't drive
/// on a tick its rider hops off.
pub fn hops_off(input: &ControlInput) -> bool {
    input.just_pressed(Action::Jump)
}

/// What a bug on the dragonfly still collides with: only enemies, whose
/// hits throw it off (`DoPlayerCollisionDetect` with `gCurrentDragonFly`).
pub fn dragonfly_rider_mask() -> LayerMask {
    CollisionKind::Enemy.into()
}

/// Where a riding bug's model is in the world, given the ride's joint
/// matrix (`FindJointFullMatrix`, which includes the ride's scale) and the
/// ride's scale: the seat in the bug's own scale, on the joint with the
/// ride's scale taken out.
pub fn seat_model_matrix(joint: Affine3A, ride_scale: f32, seat: Vec3) -> Affine3A {
    joint
        * Affine3A::from_scale(Vec3::splat(PLAYER_BUG_SCALE / ride_scale))
        * Affine3A::from_translation(seat)
}

/// Takes players on their rides. Runs with the holds, so that a ride's
/// trigger, which goes off during the player's move, takes effect for the
/// next tick.
///
/// Port of the player's half of `DoTrig_WaterBug` and `DoTrig_DragonFly`.
pub(super) fn mount_rides(
    mut mounts: MessageReader<MountRide>,
    mut commands: Commands,
    mut players: Query<(&PlayerForm, &mut BugState, Has<Dying>), With<Player>>,
) {
    for mount in mounts.read() {
        let Ok((form, mut state, dying)) = players.get_mut(mount.player) else {
            continue;
        };
        if *form != PlayerForm::Bug || dying {
            continue;
        }
        *state = mount.riding.kind.state();
        commands
            .entity(mount.player)
            .insert((mount.riding, HideShadow));
    }
}

/// Lets the ride go once its player is no longer riding it, whatever took
/// it off, and tells the ride.
pub(super) fn leave_rides(
    mut commands: Commands,
    mut left: MessageWriter<LeftRide>,
    players: Query<(Entity, &Riding, &PlayerForm, &BugState), With<Player>>,
) {
    for (player, riding, form, state) in &players {
        if *form == PlayerForm::Bug && RideKind::of_state(*state) == Some(riding.kind) {
            continue;
        }
        commands.entity(player).remove::<(Riding, HideShadow)>();
        left.write(LeftRide {
            player,
            ride: riding.ride,
        });
    }
}

/// Seats each riding bug on its ride, once the ride has driven this tick:
/// the player's root moves to the seat and keeps its heading, and the model
/// takes the ride's pose under it. A bug whose ride has gone falls off it.
///
/// Port of the placement in `MovePlayerBug_RideWaterBug` and
/// `MovePlayerBug_RideDragonFly`.
#[allow(clippy::type_complexity)]
pub(super) fn seat_riders(
    mut players: Query<(&Riding, &mut BugState, &PlayerModel, &mut Transform), With<Player>>,
    rides: Query<(Option<&EnemyModel>, Option<&RideModel>)>,
    mut transforms: ParamSet<(
        Query<(&Transform, Option<&SkeletonRig>), Without<Player>>,
        Query<&mut Transform, Without<Player>>,
    )>,
) {
    for (riding, mut state, model, mut transform) in &mut players {
        if RideKind::of_state(*state) != Some(riding.kind) {
            continue;
        }
        let Ok((enemy_model, ride_model)) = rides.get(riding.ride) else {
            *state = BugState::Fall;
            continue;
        };
        let Some((joint, scale)) = joint_matrix(
            &transforms.p0(),
            riding.ride,
            ride_model.map(|m| m.0).or(enemy_model.map(|m| m.0)),
            riding.joint,
        ) else {
            continue;
        };
        let matrix = seat_model_matrix(joint, scale, riding.seat);
        let (origin, local) = split_eaten_matrix(matrix, transform.rotation);
        transform.translation = origin;
        if let Ok(mut model_transform) = transforms.p1().get_mut(model.0) {
            *model_transform = local;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<Messages<MountRide>>();
        world.init_resource::<Messages<LeftRide>>();
        world
    }

    fn mount(world: &mut World, player: Entity, ride: Entity, kind: RideKind) {
        world.write_message(MountRide {
            player,
            riding: Riding {
                ride,
                kind,
                joint: 0,
                seat: Vec3::new(0.0, 30.0, 40.0),
            },
        });
        world.run_system_once(mount_rides).expect("the system runs");
        world.resource_mut::<Messages<MountRide>>().clear();
    }

    fn leave(world: &mut World) -> Vec<LeftRide> {
        world.run_system_once(leave_rides).expect("the system runs");
        world.resource_mut::<Messages<LeftRide>>().drain().collect()
    }

    #[test]
    fn the_bug_mounts_and_the_ride_hears_when_it_leaves() {
        let mut world = world();
        let player = world.spawn((Player, PlayerForm::Bug, BugState::Walk)).id();
        let ride = world.spawn_empty().id();
        mount(&mut world, player, ride, RideKind::WaterBug);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::RideWaterBug));
        assert!(world.get::<Riding>(player).is_some());
        assert!(world.get::<HideShadow>(player).is_some());
        assert!(leave(&mut world).is_empty());

        // Hopping off, a hold or a death all end the riding state.
        world.entity_mut(player).insert(BugState::Jump);
        assert_eq!(leave(&mut world), vec![LeftRide { player, ride }]);
        assert!(world.get::<Riding>(player).is_none());
        assert!(world.get::<HideShadow>(player).is_none());
        assert!(leave(&mut world).is_empty());
    }

    #[test]
    fn neither_the_ball_nor_a_killed_bug_mounts() {
        let mut world = world();
        let ball = world
            .spawn((Player, PlayerForm::Ball, BugState::RollUp))
            .id();
        let dead = world
            .spawn((
                Player,
                PlayerForm::Bug,
                BugState::Death,
                Dying { timer: 1.0 },
            ))
            .id();
        let ride = world.spawn_empty().id();
        mount(&mut world, ball, ride, RideKind::DragonFly);
        mount(&mut world, dead, ride, RideKind::DragonFly);
        assert_eq!(world.get::<BugState>(ball), Some(&BugState::RollUp));
        assert_eq!(world.get::<BugState>(dead), Some(&BugState::Death));
        assert!(world.get::<Riding>(ball).is_none());
        assert!(world.get::<Riding>(dead).is_none());
    }

    #[test]
    fn the_seat_is_in_the_bugs_own_scale_on_the_joint() {
        let joint = Affine3A::from_scale_rotation_translation(
            Vec3::splat(2.0),
            Quat::IDENTITY,
            Vec3::new(10.0, 20.0, 30.0),
        );
        let seat = seat_model_matrix(joint, 2.0, Vec3::new(0.0, 8.0, 55.0));
        let origin = seat.transform_point3(Vec3::ZERO);
        let expected = Vec3::new(10.0, 20.0, 30.0) + Vec3::new(0.0, 8.0, 55.0) * PLAYER_BUG_SCALE;
        assert!(origin.abs_diff_eq(expected, 1e-3), "{origin}");
        // The ride's scale is taken out: the bug keeps its own.
        let scale = seat.matrix3.x_axis.length();
        assert!((scale - PLAYER_BUG_SCALE).abs() < 1e-4);
    }
}
