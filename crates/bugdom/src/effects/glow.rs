//! The material of glowing effects (`STATUS_BIT_GLOW`), drawn in the
//! original's fixed order after the other blended surfaces.
//!
//! The original sorts what it draws by `RenderModifiers.drawOrder`
//! (original/src/Headers/renderer.h): objects and water
//! (`kDrawOrder_Default`), then ripples (`kDrawOrder_Ripples`), then glowy
//! particles (`kDrawOrder_GlowyParticles`). Bevy sorts blended meshes back
//! to front by the centre of their bounds plus [`Material::depth_bias`],
//! so a large water patch whose centre is nearer the camera than a splash
//! would be drawn over it. This material's depth bias only moves it in that
//! sort (unlike [`StandardMaterial::depth_bias`], which also offsets the
//! rasterised depth), so a bias larger than any view distance puts each
//! category after the ones before it.

use bevy::asset::{embedded_asset, embedded_path};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

/// Sort offsets that put each kind of effect after everything of a lower
/// `kDrawOrder_*` (original/src/Headers/renderer.h), in world units of view
/// depth. Everything else, water included, is at 0 (`kDrawOrder_Default`).
/// Each step is larger than any level's far plane
/// ([`crate::level::LevelTypeSettings::yon`], at most 4200), so distance
/// can never outweigh it.
pub mod draw_order {
    /// `kDrawOrder_Ripples`
    pub const RIPPLES: f32 = 10_000.0;
    /// `kDrawOrder_GlowyParticles`
    pub const GLOWY_PARTICLES: f32 = 20_000.0;
}

pub(super) fn plugin(app: &mut App) {
    embedded_asset!(app, "glow.wgsl");
    app.add_plugins(MaterialPlugin::<GlowMaterial>::default());
}

/// An unlit, additive, two-sided material that is depth tested but doesn't
/// write depth (`STATUS_BIT_GLOW | STATUS_BIT_NOZWRITE | STATUS_BIT_NOFOG |
/// STATUS_BIT_DONTCULL`).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct GlowMaterial {
    /// Multiplies the texture and the vertex colour. Its alpha scales what
    /// is added.
    #[uniform(0)]
    pub color: LinearRgba,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
    /// Where it is drawn among the blended surfaces: one of
    /// [`draw_order`]'s constants.
    pub draw_order: f32,
}

impl Material for GlowMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            bevy::asset::AssetPath::from_path_buf(embedded_path!("glow.wgsl"))
                .with_source("embedded"),
        )
    }

    fn alpha_mode(&self) -> AlphaMode {
        // Blended alpha modes don't write depth.
        AlphaMode::Add
    }

    fn depth_bias(&self) -> f32 {
        self.draw_order
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // `STATUS_BIT_DONTCULL`
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::draw_order::*;
    use crate::level::{CurrentLevel, NUM_LEVELS};

    #[test]
    fn draw_orders_follow_the_original_and_outweigh_distance() {
        const { assert!(RIPPLES > 0.0) };
        const { assert!(GLOWY_PARTICLES > RIPPLES) };
        for level in 0..NUM_LEVELS {
            let yon = CurrentLevel(level).def().settings().yon();
            assert!(RIPPLES > yon, "level {level}");
            assert!(GLOWY_PARTICLES - RIPPLES > yon, "level {level}");
        }
    }
}
