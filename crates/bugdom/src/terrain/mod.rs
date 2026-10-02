//! The level's terrain: built whole when the level starts, as one chunk
//! entity per supertile and layer, drawn with the tile texture array.
//!
//! Replaces the original's scrolling supertile cache
//! (original/src/Terrain/Terrain.c); see docs/design/phase2-engine-core.md.

mod lighting;
mod map;
mod material;
mod mesh;

use bevy::prelude::*;
use bugdom_formats::terrain::Item;

pub use map::{
    LayerKind, MAP_TO_WORLD, NO_CEILING_HEIGHT, PlaneEquation, SUPERTILE_TILES, TILE_SIZE,
    TerrainLayer, TerrainMap, face_normal,
};
pub use material::TerrainMaterial;

use crate::assets::terrain::TerrainAsset;
use crate::level::CurrentLevel;
use crate::state::{AppState, LevelAssets};

/// Map item type that marks the player's start (`MAP_ITEM_MYSTARTCOORD`).
const ITEM_START_COORD: u16 = 0;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(material::plugin).add_systems(
            OnEnter(AppState::InGame),
            spawn_terrain.in_set(TerrainSystems::Spawn),
        );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum TerrainSystems {
    /// Inserts [`TerrainMap`] and [`PlayerStart`] and spawns the chunks.
    Spawn,
}

/// One supertile of one layer.
#[derive(Component, Debug, Clone, Copy)]
pub struct TerrainChunk {
    pub layer: LayerKind,
    pub super_col: usize,
    pub super_row: usize,
}

/// Where the player starts the level (`gMyStartX`, `gMyStartZ`, `gMyStartAim`).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PlayerStart {
    /// World x and z.
    pub position: Vec2,
    /// Direction in eighths of a turn.
    pub aim: u8,
}

impl PlayerStart {
    /// Finds the start item, or uses the origin if the map has none.
    ///
    /// Port of `FindMyStartCoordItem` (original/src/Terrain/Terrain2.c).
    pub fn find(items: &[Item]) -> Self {
        match items.iter().find(|item| item.kind == ITEM_START_COORD) {
            Some(item) => Self {
                position: Vec2::new(f32::from(item.x), f32::from(item.z)) * MAP_TO_WORLD,
                aim: item.params[0],
            },
            None => {
                warn!("The terrain has no start item; starting at the origin");
                Self {
                    position: Vec2::ZERO,
                    aim: 0,
                }
            }
        }
    }

    /// The start direction as a yaw (rotation about +Y) in radians, as
    /// `InitPlayerAtStartOfLevel` turns it (original/src/Player/MyGuy.c).
    pub fn yaw(&self) -> f32 {
        f32::from(self.aim) * (std::f32::consts::TAU / 8.0)
    }
}

/// Builds the terrain for the level: the map resource, prelit chunk meshes
/// and the tile texture array.
///
/// Port of the load-time parts of `LoadPlayfield` (original/src/System/File.c),
/// `DoItemShadowCasting` and `BuildTerrainSuperTile`, done for every
/// supertile at once.
fn spawn_terrain(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    level_assets: Res<LevelAssets>,
    terrains: Res<Assets<TerrainAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let Some(terrain) = terrains.get(&level_assets.terrain) else {
        error!("The level's terrain is not loaded");
        return;
    };
    let settings = level.def().settings();
    let mut map = TerrainMap::new(terrain, settings.has_ceiling);
    let [main_light, _] = settings.fill_directions();
    lighting::cast_item_shadows(&mut map, &terrain.items, main_light);

    let tiles = images.add(mesh::build_tile_array(&terrain.tile_images));
    let material = materials.add(material::terrain_material(tiles));
    let lights = lighting::VertexLights::new(settings);
    let (supertiles_wide, supertiles_deep) = map.supertiles();
    for layer in map.layers() {
        let colors = lighting::light_layer(&map, layer, &lights);
        for super_row in 0..supertiles_deep {
            for super_col in 0..supertiles_wide {
                let chunk = mesh::build_chunk(&map, layer, &colors, super_col, super_row);
                commands.spawn((
                    Name::new(format!("Terrain {:?} {super_col},{super_row}", layer.kind)),
                    TerrainChunk {
                        layer: layer.kind,
                        super_col,
                        super_row,
                    },
                    Mesh3d(meshes.add(chunk)),
                    MeshMaterial3d(material.clone()),
                    DespawnOnExit(AppState::InGame),
                ));
            }
        }
    }

    commands.insert_resource(PlayerStart::find(&terrain.items));
    commands.insert_resource(map);
}
