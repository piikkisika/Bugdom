//! The Hive's honey tubes: lattice tubes with honey flowing inside.
//!
//! Port of `AddHoneyTube`, `MoveHoneyTube` and
//! `UpdateHoneyTubeTextureAnimation` (original/src/Items/Items2.c).

use std::f32::consts::TAU;

use bevy::math::Affine2;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use super::kind as item;
use super::scenery::{StaticObject, misc, on_level};
use super::{ItemSpawn, RegisterItemKind};
use crate::collision::CollisionBox;
use crate::level::{CurrentLevel, LevelType};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectMaterial, ObjectModel, Shading};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::HONEY_TUBE, add_honey_tube)
        .init_resource::<HoneyFlow>()
        .add_systems(OnEnter(AppState::InGame), reset_honey_flow)
        .add_systems(
            Update,
            (flow_honey, show_tube_back_faces).run_if(in_state(AppState::InGame)),
        );
}

/// `HIVE_MObjType_BentTube`; the squiggly, straight and tapered tubes
/// follow it.
const BENT_TUBE_MODEL: usize = 20;
/// `HIVE_MObjType_SquiggleTube`, which the original no longer uses.
const SQUIGGLE_TUBE: u8 = 1;
/// The straight tube, used in place of the squiggly one.
const STRAIGHT_TUBE: u8 = 2;
/// Which of a tube's meshes is the honey; mesh 0 is the lattice.
const HONEY_MESH: usize = 1;

/// A tube's scale at size 0; each step of `params[2]` adds half of it.
const TUBE_SCALE: f32 = 3.0;
const TUBE_SIZE_STEP: f32 = 0.5;
/// How fast honey flows along the tubes, in texture lengths per second.
const HONEY_FLOW_SPEED: f32 = 0.6;

/// A honey tube, whose honey [`flow_honey`] moves.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HoneyTube;

/// How far the honey has flowed along every tube, as a texture offset.
/// The original scrolls the shared meshes' UVs, so all tubes flow
/// together.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
struct HoneyFlow(f32);

/// The tube's object type and size factor for its `params`: `params[0]`
/// is the type, `params[2]` the size.
fn tube_shape(params: [u8; 4]) -> (usize, f32) {
    let mut tube = params[0];
    if tube == SQUIGGLE_TUBE {
        tube = STRAIGHT_TUBE;
    }
    let size = f32::from(params[2]) * TUBE_SIZE_STEP + 1.0;
    (BENT_TUBE_MODEL + usize::from(tube), size)
}

/// Port of `AddHoneyTube`. `params[1]` turns it by quarter turns.
fn add_honey_tube(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive], "Honey tube") {
        return false;
    }
    let (model, s) = tube_shape(spawn.params);
    let tube = StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level1, model),
        shading: Shading::Lit,
        yaw: f32::from(spawn.params[1]) * (TAU / 4.0),
        scale: TUBE_SCALE * s,
        boxes: vec![CollisionBox::new(
            400.0 * s,
            0.0,
            -70.0 * s,
            70.0 * s,
            70.0 * s,
            -70.0 * s,
        )],
        kinds: misc(),
    }
    .spawn("Honey tube", &mut commands, &mut models, &spawn);
    // `MoveHoneyTube` only tracks the item, which `DespawnOutOfRange` does.
    commands.entity(tube).insert(HoneyTube);
    true
}

fn reset_honey_flow(mut flow: ResMut<HoneyFlow>) {
    *flow = HoneyFlow::default();
}

/// The materials of each tube's meshes, as `(mesh index, material)`.
fn tube_materials<'a>(
    tubes: impl Iterator<Item = &'a Children> + 'a,
    models: &'a Query<&Children, With<ObjectModel>>,
    meshes: &'a Query<&MeshMaterial3d<ObjectMaterial>>,
) -> impl Iterator<Item = (usize, AssetId<ObjectMaterial>)> + 'a {
    tubes
        .flat_map(|children| children.iter())
        .filter_map(|child| models.get(child).ok())
        .flat_map(|parts| parts.iter().enumerate())
        .filter_map(|(index, part)| Some((index, meshes.get(part).ok()?.id())))
}

/// Port of `UpdateHoneyTubeTextureAnimation`: scrolls the honey inside
/// every tube.
fn flow_honey(
    time: Res<Time>,
    level: Res<CurrentLevel>,
    mut flow: ResMut<HoneyFlow>,
    tubes: Query<&Children, With<HoneyTube>>,
    models: Query<&Children, With<ObjectModel>>,
    meshes: Query<&MeshMaterial3d<ObjectMaterial>>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    if level.def().level_type != LevelType::Hive {
        return;
    }
    // Wrapped, so that precision doesn't run out on a long level.
    flow.0 = (flow.0 + HONEY_FLOW_SPEED * time.delta_secs()).fract_gl();
    // Each tube type's honey has its own material, shared by every tube of
    // that type.
    let honey: HashSet<_> = tube_materials(tubes.iter(), &models, &meshes)
        .filter(|&(index, _)| index == HONEY_MESH)
        .map(|(_, id)| id)
        .collect();
    let transform = Affine2::from_translation(Vec2::new(0.0, flow.0));
    for id in honey {
        if let Some(mut material) = materials.get_mut(id) {
            material.base.uv_transform = transform;
        }
    }
}

/// Draws the back of the tubes' faces too (`STATUS_BIT_KEEPBACKFACES`), so
/// that the far side of the lattice shows through. Like the original's
/// one-sided lighting, back faces keep the front's normals.
fn show_tube_back_faces(
    tubes: Query<&Children, (With<HoneyTube>, Changed<Children>)>,
    models: Query<&Children, With<ObjectModel>>,
    meshes: Query<&MeshMaterial3d<ObjectMaterial>>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    let ids: HashSet<_> = tube_materials(tubes.iter(), &models, &meshes)
        .map(|(_, id)| id)
        .collect();
    for id in ids {
        let needs_change = materials
            .get(id)
            .is_some_and(|material| material.base.cull_mode.is_some());
        if needs_change && let Some(mut material) = materials.get_mut(id) {
            material.base.cull_mode = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squiggly_tubes_are_straight() {
        assert_eq!(tube_shape([0, 0, 0, 0]), (BENT_TUBE_MODEL, 1.0));
        assert_eq!(tube_shape([1, 0, 0, 0]).0, BENT_TUBE_MODEL + 2);
        assert_eq!(tube_shape([3, 2, 4, 0]), (BENT_TUBE_MODEL + 3, 3.0));
    }
}
