//! Asset loaders for the original game data, read through `bugdom_formats`.
//!
//! The original files are served from the `original://` asset source, which
//! points at the `original/Data` directory of the submodule, e.g.
//! `original://Skeletons/Ant.skeleton.rsrc`. When the converter to open
//! formats exists, only these loaders change; gameplay code keeps using the
//! same asset types.

pub mod image;
pub mod model;
pub mod skeleton;
pub mod sound;
pub mod terrain;

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;

/// Name of the asset source that serves the original game data.
pub const ORIGINAL_SOURCE: &str = "original";

/// The asset path of a file in the original data directory, e.g.
/// `original_path("Terrain/Lawn.ter.rsrc")`.
pub fn original_path(path: &str) -> String {
    format!("{ORIGINAL_SOURCE}://{path}")
}

/// Registers the `original://` asset source. Asset sources must exist before
/// Bevy's `AssetPlugin` is built, so add this before `DefaultPlugins`.
pub struct OriginalDataSourcePlugin;

impl Plugin for OriginalDataSourcePlugin {
    fn build(&self, app: &mut App) {
        // Development builds read the submodule in place. A packaged build
        // will need a configurable location; see docs/PLAN.md (asset strategy).
        let dir = bugdom_formats::original_data_dir();
        app.register_asset_source(
            ORIGINAL_SOURCE,
            AssetSourceBuilder::platform_default(&dir.to_string_lossy(), None),
        );
    }
}

/// Registers the asset types and loaders for the original file formats.
pub struct OriginalAssetsPlugin;

impl Plugin for OriginalAssetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            image::plugin,
            model::plugin,
            skeleton::plugin,
            sound::plugin,
            terrain::plugin,
        ));
    }
}

/// Reads the whole asset into memory. The original files are small and the
/// parsers work on byte slices.
async fn read_all(reader: &mut dyn bevy::asset::io::Reader) -> Result<Vec<u8>, BevyError> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(bytes)
}
