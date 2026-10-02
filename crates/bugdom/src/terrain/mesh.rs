//! Chunk meshes and the tile texture array.
//!
//! Each tile gets its own four vertices, so it can carry its own texture
//! layer and orientation in its UVs: `UV_0` is the position within the tile
//! image and `UV_1.x` the image's layer in the texture array.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bugdom_formats::terrain::{SplitMode, TILE_IMAGE_SIZE, TileImage};

use super::map::{LayerKind, SUPERTILE_TILES, TILE_SIZE, TerrainLayer, TerrainMap};

/// Corner order of a tile's vertices: far left, far right, near right, near
/// left ("far" is the lower z).
const CORNERS: [(usize, usize); 4] = [(0, 0), (0, 1), (1, 1), (1, 0)];

/// The two triangles of a floor tile for each split, as corner indices,
/// counter-clockwise seen from above. From `gTileTriangles*` and
/// `gTileTriangleWinding` (original/src/Terrain/Terrain.c).
const FLOOR_TRIANGLES_BACKWARD: [[u16; 3]; 2] = [[3, 2, 0], [2, 1, 0]];
const FLOOR_TRIANGLES_FORWARD: [[u16; 3]; 2] = [[1, 0, 3], [2, 1, 3]];

/// Builds the mesh of one supertile of a layer. `colors` holds the lit
/// colour of every vertex of the layer, as display values.
pub fn build_chunk(
    map: &TerrainMap,
    layer: &TerrainLayer,
    colors: &[Vec3],
    super_col: usize,
    super_row: usize,
) -> Mesh {
    let tiles = SUPERTILE_TILES * SUPERTILE_TILES;
    let mut positions = Vec::with_capacity(tiles * 4);
    let mut normals = Vec::with_capacity(tiles * 4);
    let mut uvs = Vec::with_capacity(tiles * 4);
    let mut layers = Vec::with_capacity(tiles * 4);
    let mut vertex_colors = Vec::with_capacity(tiles * 4);
    let mut indices = Vec::with_capacity(tiles * 6);

    for row in super_row * SUPERTILE_TILES..(super_row + 1) * SUPERTILE_TILES {
        for col in super_col * SUPERTILE_TILES..(super_col + 1) * SUPERTILE_TILES {
            let tile = layer.tiles[row * map.width + col];
            let base = positions.len() as u16;
            for (dr, dc) in CORNERS {
                let (r, c) = (row + dr, col + dc);
                positions.push([
                    c as f32 * TILE_SIZE,
                    map.height(layer, r, c),
                    r as f32 * TILE_SIZE,
                ]);
                normals.push(map.vertex_normal(layer, r, c).to_array());
                let (u, v) = tile.source_pixel(dc, dr, 2);
                uvs.push([u as f32, v as f32]);
                layers.push([f32::from(tile.image), 0.0]);
                let [red, green, blue] = colors[r * (map.width + 1) + c].to_array();
                // The original's colours are display values; Bevy's vertex
                // colours are linear.
                vertex_colors.push(Color::srgb(red, green, blue).to_linear().to_f32_array());
            }
            let triangles = match layer.split_modes[row * map.width + col] {
                SplitMode::Backward => FLOOR_TRIANGLES_BACKWARD,
                _ => FLOOR_TRIANGLES_FORWARD,
            };
            for mut triangle in triangles {
                if layer.kind == LayerKind::Ceiling {
                    // The ceiling faces down, so its winding is reversed.
                    triangle.reverse();
                }
                indices.extend(triangle.map(|i| base + i));
            }
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, layers)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vertex_colors)
    .with_inserted_indices(Indices::U16(indices))
}

/// Packs the tile images into an sRGB texture array with a full mip chain,
/// filtered linearly with mipmaps.
pub fn build_tile_array(images: &[TileImage]) -> Image {
    let mip_levels = TILE_IMAGE_SIZE.ilog2() + 1;
    let mut data = Vec::new();
    for image in images {
        // Layer-major order: every mip of a layer before the next layer.
        let mut level = image.rgba.clone();
        let mut size = TILE_IMAGE_SIZE;
        data.extend_from_slice(&level);
        while size > 1 {
            level = downsample(&level, size);
            size /= 2;
            data.extend_from_slice(&level);
        }
    }

    let layers = images.len().max(1);
    // `Image::new` checks the data against the base level only, so the mip
    // chain goes in afterwards.
    let mut array = Image::new_fill(
        Extent3d {
            width: TILE_IMAGE_SIZE as u32,
            height: TILE_IMAGE_SIZE as u32,
            depth_or_array_layers: layers as u32,
        },
        TextureDimension::D2,
        &[255, 0, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    if !images.is_empty() {
        array.data = Some(data);
        array.texture_descriptor.mip_level_count = mip_levels;
    }
    array.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    array
}

/// Halves an sRGB RGBA8 image with a 2×2 box filter, averaging in linear space.
fn downsample(rgba: &[u8], size: usize) -> Vec<u8> {
    let half = size / 2;
    let mut out = Vec::with_capacity(half * half * 4);
    for y in 0..half {
        for x in 0..half {
            let mut sum = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y * 2 + dy) * size + x * 2 + dx) * 4;
                for channel in 0..3 {
                    sum[channel] += srgb_to_linear(rgba[i + channel]);
                }
                sum[3] += f32::from(rgba[i + 3]) / 255.0;
            }
            out.extend(sum[..3].iter().map(|&c| linear_to_srgb(c / 4.0)));
            out.push((sum[3] / 4.0 * 255.0).round() as u8);
        }
    }
    out
}

fn srgb_to_linear(value: u8) -> f32 {
    Color::srgb_u8(value, 0, 0).to_linear().red
}

fn linear_to_srgb(value: f32) -> u8 {
    Color::linear_rgb(value, 0.0, 0.0).to_srgba().to_u8_array()[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_triangles_face_up() {
        let corner = |i: u16| {
            let (r, c) = CORNERS[usize::from(i)];
            Vec3::new(c as f32, 0.0, r as f32)
        };
        for triangle in FLOOR_TRIANGLES_BACKWARD
            .iter()
            .chain(&FLOOR_TRIANGLES_FORWARD)
        {
            let [a, b, c] = triangle.map(corner);
            assert!((b - a).cross(c - a).y > 0.0, "{triangle:?}");
        }
    }

    #[test]
    fn downsampling_a_flat_colour_keeps_it() {
        let rgba = [200u8, 100, 50, 255].repeat(16);
        assert_eq!(downsample(&rgba, 4), [200u8, 100, 50, 255].repeat(4));
    }
}
