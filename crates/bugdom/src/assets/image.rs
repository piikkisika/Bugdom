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
/// (`kRendererTextureFlags_SolidBlackIsAlpha`, e.g. fences and the infobar's
/// sprites); that is applied by the code that uses them, with
/// [`black_to_transparent`] or [`TgaSettings`].
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

/// How a TGA image is adapted for its use once it has loaded: the texture
/// flags the original passes along with some of its images.
///
/// This was meant to be `TgaLoader`'s loader settings (`load_with_settings`),
/// but Bevy needs loader settings to implement serde's traits, and serde is
/// not a direct dependency of this crate. Until it is, [`TgaSettings::apply`]
/// is run on the image after it has loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TgaSettings {
    /// Pure black is transparent ([`black_to_transparent`]): the infobar's
    /// sprite masks (`LoadSpriteResources`) and
    /// `kRendererTextureFlags_SolidBlackIsAlpha`.
    pub black_is_transparent: bool,
    /// Transparent texels take their opaque neighbours' colour
    /// ([`bleed_edges`]), so that bilinear filtering doesn't blend black
    /// into the edges of a scaled-up sprite.
    pub bleed_edges: bool,
    /// Clamps the texture at its edges in both directions rather than
    /// repeating it (`kRendererTextureFlags_ClampBoth`).
    pub clamp: bool,
}

impl TgaSettings {
    /// Applies the settings to an image that [`TgaLoader`] loaded.
    pub fn apply(&self, image: &mut Image) {
        let (width, height) = (image.width() as usize, image.height() as usize);
        if let Some(data) = image.data.as_mut() {
            if self.black_is_transparent {
                black_to_transparent(data);
            }
            if self.bleed_edges {
                bleed_edges(data, width, height);
            }
        }
        if self.clamp
            && let ImageSampler::Descriptor(sampler) = &mut image.sampler
        {
            sampler.address_mode_u = ImageAddressMode::ClampToEdge;
            sampler.address_mode_v = ImageAddressMode::ClampToEdge;
        }
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

/// Gives each fully transparent texel the average colour of its opaque
/// neighbours (of the eight around it), leaving its alpha at 0.
///
/// With bilinear filtering, a texel between an opaque one and a transparent
/// one blends both colours. Without this, the transparent texels' black
/// would darken the edge of every scaled-up sprite; the original never
/// filters the sprites on their own, only the composited bar, so it has
/// no such fringe.
pub fn bleed_edges(rgba: &mut [u8], width: usize, height: usize) {
    if rgba.len() != width * height * 4 {
        return;
    }
    let source = rgba.to_vec();
    let texel = |x: usize, y: usize| &source[(y * width + x) * 4..][..4];
    for y in 0..height {
        for x in 0..width {
            if texel(x, y)[3] != 0 {
                continue;
            }
            let mut sum = [0u32; 3];
            let mut count = 0;
            for ny in y.saturating_sub(1)..(y + 2).min(height) {
                for nx in x.saturating_sub(1)..(x + 2).min(width) {
                    let neighbour = texel(nx, ny);
                    if neighbour[3] != 0 {
                        for (total, channel) in sum.iter_mut().zip(neighbour) {
                            *total += u32::from(*channel);
                        }
                        count += 1;
                    }
                }
            }
            let out = &mut rgba[(y * width + x) * 4..][..3];
            for (channel, total) in out.iter_mut().zip(sum) {
                if let Some(average) = total.checked_div(count) {
                    *channel = average as u8;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bleeding_colours_transparent_texels_but_keeps_them_transparent() {
        // A row: red, transparent black, transparent black, blue.
        let mut rgba = vec![
            255, 0, 0, 255, //
            0, 0, 0, 0, //
            0, 0, 0, 0, //
            0, 0, 255, 255,
        ];
        bleed_edges(&mut rgba, 4, 1);
        assert_eq!(&rgba[4..8], &[255, 0, 0, 0]);
        assert_eq!(&rgba[8..12], &[0, 0, 255, 0]);
        // Opaque texels are untouched.
        assert_eq!(&rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn bleeding_averages_the_opaque_neighbours_only() {
        // 3×1: white, transparent, black-but-opaque.
        let mut rgba = vec![
            200, 100, 50, 255, //
            0, 0, 0, 0, //
            0, 0, 0, 255,
        ];
        bleed_edges(&mut rgba, 3, 1);
        assert_eq!(&rgba[4..8], &[100, 50, 25, 0]);
    }

    #[test]
    fn settings_make_black_transparent_and_clamp() {
        let mut image = rgba8_image(
            2,
            1,
            vec![0, 0, 0, 255, 10, 20, 30, 255],
            ImageAddressMode::Repeat,
            ImageAddressMode::Repeat,
        );
        TgaSettings {
            black_is_transparent: true,
            bleed_edges: true,
            clamp: true,
        }
        .apply(&mut image);
        assert_eq!(
            image.data.as_deref(),
            Some(&[10, 20, 30, 0, 10, 20, 30, 255][..])
        );
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("the sampler is not a descriptor");
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::ClampToEdge);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::ClampToEdge);
    }
}
