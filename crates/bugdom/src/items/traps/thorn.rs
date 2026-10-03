//! The thorn bush: a solid bush on the Forest levels that hurts on touch.
//!
//! Port of `AddThorn` (original/src/Items/Traps.c).

use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::super::kind as item;
use super::super::scenery::{StaticObject, on_level};
use super::super::{ItemSpawn, RegisterItemKind};
use crate::collision::{CollisionBox, CollisionKind};
use crate::combat::Damage;
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::ModelSpawner;
use crate::objects::{ModelFile, ModelRef, Shading};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::THORN, add_thorn);
}

/// `FOREST_MObjType_Thorn1`; `Thorn2` follows.
const THORN_MODEL: usize = 8;
/// `THORN_SCALE`
const THORN_SCALE: f32 = 2.2;
/// What touching a thorn bush does to the player (`Damage`).
const THORN_DAMAGE: f32 = 0.1;

/// The bush's boxes, relative to its base, before scaling: `variant` is
/// `params[0]` (the bush shape), `rot` its facing in quarter turns. The
/// original writes out a table per facing rather than rotating, and this
/// keeps it.
fn thorn_boxes(variant: u8, rot: u8) -> Vec<CollisionBox> {
    // In the order `CollisionBox::new` takes: top, bottom, left, right,
    // front, back.
    let b = |top: f32, bottom: f32, left: f32, right: f32, front: f32, back: f32| {
        CollisionBox::new(top, bottom, left, right, front, back)
    };
    let boxes = if variant != 0 {
        // A trunk with two branches.
        let trunk = b(350.0, 0.0, -20.0, 20.0, 20.0, -20.0);
        let (low, high) = (314.0, 0.0);
        let (top, bottom) = (458.0, 146.0);
        match rot {
            0 => [
                trunk,
                b(low, high, -15.0, 15.0, 215.0, 20.0),
                b(top, bottom, -15.0, 15.0, -20.0, -277.0),
            ],
            1 => [
                trunk,
                b(low, high, 20.0, 215.0, 15.0, -15.0),
                b(top, bottom, -277.0, -20.0, 15.0, -15.0),
            ],
            2 => [
                trunk,
                b(low, high, -15.0, 15.0, -20.0, -215.0),
                b(top, bottom, -15.0, 15.0, 277.0, 20.0),
            ],
            _ => [
                trunk,
                b(low, high, -215.0, -20.0, 15.0, -15.0),
                b(top, bottom, 20.0, 277.0, 15.0, -15.0),
            ],
        }
    } else {
        // A long bush in three parts, the middle one raised.
        let (top, bottom, mid_bottom) = (283.0, 0.0, 155.0);
        match rot {
            0 => [
                b(top, bottom, -35.0, 35.0, 48.0, -173.0),
                b(top, mid_bottom, -35.0, 35.0, -173.0, -298.0),
                b(top, bottom, -35.0, 35.0, -298.0, -400.0),
            ],
            1 => [
                b(top, bottom, -173.0, 48.0, 35.0, -35.0),
                b(top, mid_bottom, -298.0, -173.0, 35.0, -35.0),
                b(top, bottom, -400.0, -298.0, 35.0, -35.0),
            ],
            2 => [
                b(top, bottom, -35.0, 35.0, 173.0, -48.0),
                b(top, mid_bottom, -35.0, 35.0, 298.0, 173.0),
                b(top, bottom, -35.0, 35.0, 400.0, 298.0),
            ],
            _ => [
                b(top, bottom, -48.0, 173.0, 35.0, -35.0),
                b(top, mid_bottom, 173.0, 298.0, 35.0, -35.0),
                b(top, bottom, 298.0, 400.0, 35.0, -35.0),
            ],
        }
    };
    boxes
        .into_iter()
        .map(|b| {
            CollisionBox::new(
                b.top * THORN_SCALE,
                b.bottom * THORN_SCALE,
                b.left * THORN_SCALE,
                b.right * THORN_SCALE,
                b.front * THORN_SCALE,
                b.back * THORN_SCALE,
            )
        })
        .collect()
}

/// Port of `AddThorn`. `params[0]` is the bush (0 or 1), `params[1]` its
/// facing in quarter turns, and bit 0 of `params[3]` faces it at random.
fn add_thorn(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let variant = spawn.params[0];
    if !on_level(&level, &[LevelType::Forest], "Thorn") || variant > 1 {
        return false;
    }
    let rot = if spawn.params[3] & 1 != 0 {
        (random.next_u32() & 3) as u8
    } else {
        spawn.params[1] & 3
    };
    let thorn = StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level1, THORN_MODEL + usize::from(variant)),
        shading: Shading::Lit,
        yaw: f32::from(rot) * FRAC_PI_2,
        scale: THORN_SCALE,
        boxes: thorn_boxes(variant, rot),
        kinds: LayerMask::from([
            CollisionKind::Misc,
            CollisionKind::HurtMe,
            CollisionKind::BlockCamera,
        ]),
    }
    .spawn("Thorn bush", &mut commands, &mut models, &spawn);
    commands.entity(thorn).insert(Damage(THORN_DAMAGE));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_bush_turns_its_boxes_a_quarter_turn_at_a_time() {
        let facing = |rot| thorn_boxes(0, rot);
        // Facing 0 it reaches toward −Z; a quarter turn counter-clockwise
        // takes −Z to −X.
        let far = |boxes: &[CollisionBox]| boxes[2];
        assert!(far(&facing(0)).back < -800.0);
        assert!(far(&facing(1)).left < -800.0);
        assert!(far(&facing(2)).front > 800.0);
        assert!(far(&facing(3)).right > 800.0);
        // The middle part is raised off the ground.
        for rot in 0..4 {
            assert!((facing(rot)[1].bottom - 155.0 * THORN_SCALE).abs() < 1e-3);
        }
    }

    #[test]
    fn the_branched_bush_keeps_its_trunk_whatever_its_facing() {
        for rot in 0..4 {
            let boxes = thorn_boxes(1, rot);
            assert_eq!(boxes.len(), 3);
            assert_eq!(boxes[0], thorn_boxes(1, 0)[0]);
        }
    }
}
