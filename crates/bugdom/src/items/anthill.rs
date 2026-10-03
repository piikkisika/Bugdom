//! Ant Hill plumbing: the water valves, the fire walls they put out, the
//! ant pipes that leak when they open, the king's water pipe, and the root
//! swings hanging from the ceiling.
//!
//! Port of `AddWaterValve`, `MoveWaterValve` and `DoTrig_WaterValve`
//! (original/src/Items/Triggers.c), `AddFireWall` and `MoveFireWall`
//! (original/src/Items/Traps.c), `AddBentAntPipe`, `AddHorizAntPipe` and
//! `MoveAntPipe` (original/src/Items/Items.c), `AddKingWaterPipe`,
//! `MoveKingWaterPipe`, `DoTrig_KingWaterPipe` and `KickKingWaterPipe`
//! (original/src/Items/Triggers2.c), and the root swing in
//! original/src/Items/Items2.c.

mod fire_wall;
mod king_pipe;
mod pipes;
mod root_swing;
mod valve;

use bevy::prelude::*;

pub use root_swing::{GrabRootSwing, RootSwing};

use crate::effects::{FULL_ALPHA, ParticleGroupDesc, ParticleGroupId, ParticleGroups};
use crate::liquids::WaterValves;
use crate::math::GameRandom;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins((
        valve::plugin,
        fire_wall::plugin,
        pipes::plugin,
        king_pipe::plugin,
        root_swing::plugin,
    ));
}

/// Object types in the Ant Hill's model file (`ANTHILL_MObjType_*`).
mod anthill_models {
    pub const WATER_VALVE_BOX: usize = 0;
    pub const WATER_VALVE_HANDLE: usize = 1;
    pub const BENT_PIPE: usize = 3;
    pub const HORIZ_PIPE: usize = 4;
    pub const KING_PIPE: usize = 5;
}

/// Whether valve `id` is open (`gValveIsOpen[id]`). The original has 255
/// valve slots; the Ant Hill's map only uses the first few, which
/// [`WaterValves`] holds, and any other is always shut.
fn valve_is_open(valves: &WaterValves, id: u8) -> bool {
    valves.0.get(usize::from(id)).copied().unwrap_or(false)
}

/// Adds a burst of `count` particles to the group in `slot`, starting a
/// new group when there is none (`VerifyParticleGroup` and
/// `NewParticleGroup`). `particle` makes the `i`th particle's position,
/// velocity and scale.
///
/// When the group fills up during the burst, `restart_when_full` sends the
/// whole burst again into a new group, as the original's `goto new_group`
/// does; it does so once, where the original would loop forever on a burst
/// bigger than a group. Without it the rest of the burst is lost and the
/// full group is kept, as when the original ignores
/// `AddParticleToGroup`'s result.
fn emit_burst(
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    slot: &mut Option<ParticleGroupId>,
    desc: ParticleGroupDesc,
    count: usize,
    restart_when_full: bool,
    mut particle: impl FnMut(&mut GameRandom, usize) -> (Vec3, Vec3, f32),
) {
    let attempts = if restart_when_full { 2 } else { 1 };
    for _ in 0..attempts {
        let group = match slot.filter(|g| groups.is_valid(*g)) {
            Some(group) => group,
            None => {
                *slot = groups.new_group(desc);
                let Some(group) = *slot else {
                    return;
                };
                group
            }
        };
        let mut full = false;
        for i in 0..count {
            let (position, velocity, scale) = particle(random, i);
            if groups.add_particle(group, position, velocity, scale, FULL_ALPHA) {
                full = true;
                break;
            }
        }
        if !full || !restart_when_full {
            return;
        }
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{MAX_PARTICLES, ParticleFlags, ParticleKind};

    const DESC: ParticleGroupDesc = ParticleGroupDesc {
        kind: ParticleKind::Sparks,
        flags: ParticleFlags::NONE,
        gravity: 0.0,
        magnetism: 0.0,
        base_scale: 10.0,
        decay_rate: 0.0,
        fade_rate: 0.0,
        texture: crate::effects::ParticleTexture::Fire,
    };

    fn count(groups: &ParticleGroups, slot: Option<ParticleGroupId>) -> usize {
        slot.and_then(|g| groups.get(g))
            .map_or(0, |g| g.particles().len())
    }

    #[test]
    fn a_burst_starts_a_group_and_keeps_it() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut slot = None;
        let at = |_: &mut GameRandom, _| (Vec3::ZERO, Vec3::Y, 1.0);
        emit_burst(&mut groups, &mut random, &mut slot, DESC, 3, true, at);
        let first = slot;
        emit_burst(&mut groups, &mut random, &mut slot, DESC, 3, true, at);
        assert_eq!(slot, first);
        assert_eq!(count(&groups, slot), 6);
    }

    #[test]
    fn a_full_group_restarts_the_burst_in_a_new_one() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut slot = None;
        let at = |_: &mut GameRandom, _| (Vec3::ZERO, Vec3::Y, 1.0);
        emit_burst(
            &mut groups,
            &mut random,
            &mut slot,
            DESC,
            MAX_PARTICLES - 2,
            true,
            at,
        );
        let first = slot;
        emit_burst(&mut groups, &mut random, &mut slot, DESC, 5, true, at);
        assert_ne!(slot, first);
        assert_eq!(count(&groups, first), MAX_PARTICLES);
        assert_eq!(count(&groups, slot), 5);
    }

    #[test]
    fn without_restarting_a_full_group_is_kept() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut slot = None;
        let at = |_: &mut GameRandom, _| (Vec3::ZERO, Vec3::Y, 1.0);
        emit_burst(
            &mut groups,
            &mut random,
            &mut slot,
            DESC,
            MAX_PARTICLES + 3,
            false,
            at,
        );
        let first = slot;
        emit_burst(&mut groups, &mut random, &mut slot, DESC, 5, false, at);
        assert_eq!(slot, first);
        assert_eq!(count(&groups, slot), MAX_PARTICLES);
    }

    #[test]
    fn unknown_valves_are_shut() {
        let mut valves = WaterValves::default();
        valves.0[2] = true;
        assert!(valve_is_open(&valves, 2));
        assert!(!valve_is_open(&valves, 1));
        assert!(!valve_is_open(&valves, 200));
    }
}
