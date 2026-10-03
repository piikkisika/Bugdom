//! Hive mechanisms: the honeycomb platforms, the detonators with the
//! firecrackers and hive doors they set off, the floor spikes, and the
//! shockwave of an exploding cherry bomb.
//!
//! Port of the honeycomb platform and detonator parts of
//! original/src/Items/Triggers.c, the firecracker, hive door and spline
//! platform parts of original/src/Items/Items2.c, and the shockwave and
//! floor spike parts of original/src/Items/Traps.c.
//!
//! Exploding firecrackers burst into shards (`QD3D_ExplodeGeometry`)
//! amid their sparks.

mod detonator;
mod firecracker;
mod platform;
mod spike;

use bevy::prelude::*;

use crate::objects::{ModelFile, ModelRef};

pub(super) fn plugin(app: &mut App) {
    app.add_plugins((
        detonator::plugin,
        firecracker::plugin,
        platform::plugin,
        spike::plugin,
    ));
}

/// The Hive's models in `MODEL_GROUP_LEVELSPECIFIC` (`HIVE_MObjType_*`).
mod model {
    use super::{ModelFile, ModelRef};

    /// `HIVE_MObjType_BrickPlatform`; the steel platform follows it.
    pub const BRICK_PLATFORM: usize = 0;
    pub const WOOD_PLATFORM: ModelRef = ModelRef::new(ModelFile::Level1, 2);
    pub const FIRECRACKER: ModelRef = ModelRef::new(ModelFile::Level1, 3);
    /// `HIVE_MObjType_DetonatorGreen`; the other colours follow.
    pub const DETONATOR_GREEN: usize = 4;
    pub const PLUNGER: ModelRef = ModelRef::new(ModelFile::Level1, 9);
    /// `HIVE_MObjType_HiveDoor_Green`; the other colours follow.
    pub const HIVE_DOOR_GREEN: usize = 10;
    /// `HIVE_MObjType_HiveDoor_GreenOpen`; the other colours follow.
    pub const HIVE_DOOR_GREEN_OPEN: usize = 15;
    pub const FLOOR_SPIKE: ModelRef = ModelRef::new(ModelFile::Level1, 25);
    /// `NIGHT_MObjType_CherryBomb`; the Night's firecracker follows it.
    pub const NIGHT_CHERRY_BOMB: usize = 7;
    /// `GLOBAL1_MObjType_ShockWave`
    pub const SHOCKWAVE: ModelRef = ModelRef::new(ModelFile::Global1, 1);
}
