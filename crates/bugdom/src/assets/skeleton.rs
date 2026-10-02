//! Skeleton files (`Skeletons/*.skeleton.rsrc` plus the `.3dmf` of the same
//! name), loaded as a [`SkeletonAsset`] ready for GPU skinning.

use std::sync::Arc;

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bugdom_formats::rsrc::ResourceFork;
use bugdom_formats::skeleton::{self, SkeletonDefinition};
use bugdom_formats::{skin, tdmf};

use super::model::{ModelPart, add_parts};

pub(super) fn plugin(app: &mut App) {
    app.init_asset::<SkeletonAsset>()
        .init_asset_loader::<SkeletonLoader>();
}

/// A skeleton definition with its skinned geometry.
#[derive(Asset, TypePath, Debug)]
pub struct SkeletonAsset {
    pub definition: Arc<SkeletonDefinition>,
    /// The skeleton's meshes, already skinned: each vertex has one joint,
    /// indexed like [`SkeletonDefinition::bones`].
    pub parts: Vec<ModelPart>,
    /// One per bone: the inverse of a translation to the bone's bind-pose
    /// position (see `bugdom_formats::skin`).
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// Radius of the bounding sphere of the skeleton's geometry at scale 1
    /// (`gSkeletonBoundingSpheres`), which, scaled, is how far objects keep
    /// from fences.
    pub radius: f32,
}

/// Loads `.skeleton.rsrc` files as [`SkeletonAsset`]s. Also reads the
/// skeleton's geometry from the `.3dmf` of the same name in the same folder,
/// as `LoadASkeleton` (original/src/Skeleton/SkeletonObj.c) does.
#[derive(Default, TypePath)]
pub struct SkeletonLoader;

const EXTENSION: &str = "skeleton.rsrc";

impl AssetLoader for SkeletonLoader {
    type Asset = SkeletonAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<SkeletonAsset, BevyError> {
        let bytes = super::read_all(reader).await?;
        let definition = skeleton::parse(&ResourceFork::from_apple_double(&bytes)?)?;

        let file_name = load_context
            .path()
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let name = file_name
            .strip_suffix(EXTENSION)
            .and_then(|n| n.strip_suffix('.'))
            .ok_or_else(|| format!("{file_name} does not end in .{EXTENSION}"))?
            .to_owned();
        let model_path = load_context
            .path()
            .resolve_embed_str(&format!("{name}.3dmf"))?;
        let model = tdmf::parse(&load_context.read_asset_bytes(model_path).await?)?;

        let radius = tdmf::BoundingSphere::of_meshes(&model.meshes).radius;
        let skinned = skin::bind(&definition, &model)?;
        // Only RootSwing relies on repeating UVs; every other skeleton clamps
        // to avoid seams at the edges of alpha-tested textures.
        let force_clamp = !name.eq_ignore_ascii_case("RootSwing");
        let parts = add_parts(&model, Some(&skinned), force_clamp, load_context);

        let inverse_bindposes: Vec<Mat4> = definition
            .bones
            .iter()
            .map(|bone| Mat4::from_translation(-Vec3::from(bone.coord)))
            .collect();
        let inverse_bindposes =
            load_context.add_labeled_asset("InverseBindposes", inverse_bindposes.into());

        Ok(SkeletonAsset {
            definition: Arc::new(definition),
            parts,
            inverse_bindposes,
            radius,
        })
    }

    fn extensions(&self) -> &[&str] {
        &[EXTENSION]
    }
}
