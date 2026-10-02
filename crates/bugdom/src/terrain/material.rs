//! The terrain material: the tile texture array times the prelit vertex
//! colour, unlit, with fog.
//!
//! The original draws the terrain with no lighting (`STATUS_BIT_NULLSHADER`
//! in `InitTerrainManager`, original/src/Terrain/Terrain.c) because the
//! lighting is already in the vertex colours.

use bevy::asset::{embedded_asset, embedded_path};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub(super) fn plugin(app: &mut App) {
    embedded_asset!(app, "terrain.wgsl");
    app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
}

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

/// Samples the tile texture array; see the module docs of `super::mesh` for
/// the UV layout.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct TerrainExtension {
    #[texture(100, dimension = "2d_array")]
    #[sampler(101)]
    pub tiles: Handle<Image>,
}

impl MaterialExtension for TerrainExtension {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            bevy::asset::AssetPath::from_path_buf(embedded_path!("terrain.wgsl"))
                .with_source("embedded"),
        )
    }
}

/// A terrain material for the given tile array.
pub fn terrain_material(tiles: Handle<Image>) -> TerrainMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            ..default()
        },
        extension: TerrainExtension { tiles },
    }
}
