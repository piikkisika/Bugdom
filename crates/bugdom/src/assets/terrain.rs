//! Terrain files (`Terrain/*.ter.rsrc`), loaded as parsed data. Building
//! chunk meshes and the tile texture array is the terrain plugin's job.

use std::sync::Arc;

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::prelude::*;
use bugdom_formats::rsrc::ResourceFork;
use bugdom_formats::terrain::{self, Terrain};

pub(super) fn plugin(app: &mut App) {
    app.init_asset::<TerrainAsset>()
        .init_asset_loader::<TerrainLoader>();
}

/// A parsed terrain file.
#[derive(Asset, TypePath, Debug, Clone, Deref)]
pub struct TerrainAsset(pub Arc<Terrain>);

/// Loads `.ter.rsrc` files as [`TerrainAsset`]s.
#[derive(Default, TypePath)]
pub struct TerrainLoader;

impl AssetLoader for TerrainLoader {
    type Asset = TerrainAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<TerrainAsset, BevyError> {
        let bytes = super::read_all(reader).await?;
        let terrain = terrain::parse(&ResourceFork::from_apple_double(&bytes)?)?;
        Ok(TerrainAsset(Arc::new(terrain)))
    }

    fn extensions(&self) -> &[&str] {
        &["ter.rsrc"]
    }
}
