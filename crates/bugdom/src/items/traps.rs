//! Traps: the foot, thorn bush, stump and its hive, and the rolling boulder.
//!
//! Port of `PrimeFoot`, `AddThorn` and `AddRollingBoulder`
//! (original/src/Items/Traps.c), and `AddStump`, `MoveStump` and
//! `RattleHive` (original/src/Items/Items.c). The bat, also in Traps.c,
//! belongs with the dragonfly ride.

mod boulder;
mod foot;
mod stump;
mod thorn;

use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins((boulder::plugin, foot::plugin, stump::plugin, thorn::plugin));
}

/// The model entity under a trap's root, which carries its rotation and
/// scale, so that the root keeps the unrotated collision boxes.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct TrapModel(Entity);
