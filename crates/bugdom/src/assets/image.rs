//! TGA images (`Images/**/*.tga`), decoded by `bugdom_formats::tga` so the
//! original's alpha handling is kept exactly.

use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bugdom_formats::tga;

pub(super) fn plugin(app: &mut App) {
    app.init_asset_loader::<TgaLoader>();
}

/// Loads `.tga` files as Bevy [`Image`]s.
///
/// Some textures in the original treat pure black as transparent
/// (`kRendererTextureFlags_SolidBlackIsAlpha`, e.g. fences); that is applied
/// by the code that uses them, with [`black_to_transparent`].
#[derive(Default, TypePath)]
pub struct TgaLoader;

impl AssetLoader for TgaLoader {
    type Asset = Image;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Image, BevyError> {
        let bytes = super::read_all(reader).await?;
        let decoded = tga::parse(&bytes)?;
        Ok(rgba8_image(
            decoded.width,
            decoded.height,
            decoded.pixels,
            ImageAddressMode::Repeat,
            ImageAddressMode::Repeat,
        ))
    }

    fn extensions(&self) -> &[&str] {
        &["tga"]
    }
}

/// Makes pure black pixels fully transparent, as `kRendererTextureFlags_SolidBlackIsAlpha`
/// does in `QD3D_LoadTextureFile` (original/src/QD3D/QD3D_Support.c).
pub fn black_to_transparent(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[..3] == [0, 0, 0] {
            pixel[3] = 0;
        }
    }
}

/// An sRGB RGBA8 image with bilinear filtering, as the original's renderer
/// uses at its default detail setting (`Render_LoadTexture` in
/// original/src/QD3D/Renderer.c).
pub(crate) fn rgba8_image(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    address_mode_u: ImageAddressMode,
    address_mode_v: ImageAddressMode,
) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u,
        address_mode_v,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}
