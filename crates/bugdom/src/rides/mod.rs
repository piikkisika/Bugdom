//! The rides: the dragonfly and the water bug, which the player rides.
//!
//! Port of original/src/Ride (DragonFly.c, WaterBug.c). The player's side
//! of riding is in `player/ride.rs`: a ride's trigger sends
//! [`MountRide`](crate::player::MountRide), the ride drives itself in
//! [`PlayerSystems::Ride`](crate::player::PlayerSystems::Ride) reading its
//! rider's controls, and hears [`LeftRide`](crate::player::LeftRide) when the
//! rider leaves.

use bevy::prelude::*;

pub mod dragonfly;
pub mod water_bug;

pub struct RidesPlugin;

impl Plugin for RidesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((dragonfly::plugin, water_bug::plugin));
    }
}
