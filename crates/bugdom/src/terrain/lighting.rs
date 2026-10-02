//! Terrain lighting, done once at load time as in the original: item shadows
//! darken the floor's vertex colours, then each vertex colour is multiplied
//! by the ambient and fill lights. The terrain is drawn unlit.

use bevy::prelude::*;
use bugdom_formats::terrain::{Item, rgb565_to_rgb};

use super::map::{MAP_TO_WORLD, TILE_SIZE, TerrainLayer, TerrainMap};
use crate::level::{AMBIENT_BRIGHTNESS, FILL_BRIGHTNESS, LevelTypeSettings};

/// How much a shadow darkens a vertex colour.
const SHADOW_FACTOR: f32 = 0.7;

/// Height of the shadow-casting items, which sets how long their shadow is.
/// Item kinds without an entry cast no shadow.
fn shadow_caster_height(kind: u16) -> Option<f32> {
    Some(match kind {
        5 => 1000.0,       // clover
        6 => 1200.0,       // grass
        7 => 2500.0,       // weed
        10 => 3000.0,      // sunflower
        11 => 2000.0,      // cosmo
        12 => 2000.0,      // poppy
        19 => 3000.0,      // cattail
        20 => 2500.0,      // duckweed
        21 => 2000.0,      // lily flower
        22 => 2000.0,      // lily pad
        23 => 2500.0,      // pond grass
        24 => 2500.0,      // reed
        45 => 1200.0,      // honey tube
        51 => 3000.0,      // tree stump
        57 | 58 => 1000.0, // ant pipes
        60 => 1200.0,      // faucet
        63 => 1200.0,      // king pipe
        _ => return None,
    })
}

/// Darkens the floor's vertex colours along the shadow of each tall item.
///
/// Port of `DoItemShadowCasting` (original/src/Terrain/Terrain2.c),
/// including its quirks: rows and columns are truncated toward zero, the last
/// vertex row and column are never shaded, and each vertex is shaded at most
/// once.
pub fn cast_item_shadows(map: &mut TerrainMap, items: &[Item], light_direction: Vec3) {
    let (width, depth) = (map.width, map.depth);
    let mut shaded = vec![false; (width + 1) * (depth + 1)];
    let light = Vec2::new(light_direction.x, light_direction.z).normalize_or_zero();
    let length_factor = 1.0 + light_direction.y;

    for item in items {
        let Some(height) = shadow_caster_height(item.kind) else {
            continue;
        };
        let from = Vec2::new(f32::from(item.x), f32::from(item.z)) * MAP_TO_WORLD;
        let to = from + light * (height * length_factor);
        let step = 1.0 / (from.distance(to) / TILE_SIZE);

        let mut t = 1.0f32;
        while t > 0.0 {
            let centre = from * (1.0 - t) + to * t;
            for row_offset in [-0.5f32, 0.0, 0.5] {
                for col_offset in [-0.5f32, 0.0, 0.5] {
                    // `as` truncates toward zero, like the C conversion.
                    let row = (centre.y / TILE_SIZE + row_offset) as i64;
                    let col = (centre.x / TILE_SIZE + col_offset) as i64;
                    if row < 0 || col < 0 || row >= depth as i64 || col >= width as i64 {
                        continue;
                    }
                    let index = row as usize * (width + 1) + col as usize;
                    if shaded[index] {
                        continue;
                    }
                    shaded[index] = true;
                    let color = &mut map.floor.vertex_colors[index];
                    *color = darken_rgb565(*color);
                }
            }
            t -= step;
        }
    }
}

/// Multiplies an RGB 5-6-5 colour by [`SHADOW_FACTOR`], converting through
/// floats exactly as `DoItemShadowCasting` does.
fn darken_rgb565(color: u16) -> u16 {
    let r = f32::from(color >> 11) / 31.0 * SHADOW_FACTOR;
    let g = f32::from((color >> 5) & 0x3f) / 63.0 * SHADOW_FACTOR;
    let b = f32::from(color & 0x1f) / 31.0 * SHADOW_FACTOR;
    ((r * 31.0) as u16) << 11 | ((g * 63.0) as u16) << 5 | (b * 31.0) as u16
}

/// The level's lights as `BuildTerrainSuperTile` applies them.
#[derive(Debug, Clone, Copy)]
pub struct VertexLights {
    ambient: Vec3,
    fills: [(Vec3, Vec3); 2],
}

impl VertexLights {
    pub fn new(settings: &LevelTypeSettings) -> Self {
        let [ambient, fill0, fill1] = settings.light_colors.map(Vec3::from);
        let [dir0, dir1] = settings.fill_directions();
        Self {
            ambient: ambient * AMBIENT_BRIGHTNESS,
            fills: [
                (fill0 * FILL_BRIGHTNESS[0], dir0),
                (fill1 * FILL_BRIGHTNESS[1], dir1),
            ],
        }
    }

    /// The lit colour of a vertex, as display values in `0.0..=1.0`.
    ///
    /// Port of the vertex colour loop in `BuildTerrainSuperTile`
    /// (original/src/Terrain/Terrain.c).
    pub fn light(&self, diffuse: u16, normal: Vec3) -> Vec3 {
        let mut light = self.ambient;
        for (color, direction) in self.fills {
            let dot = -normal.dot(direction);
            if dot > 0.0 {
                light += color * dot;
            }
        }
        (Vec3::from(rgb565_to_rgb(diffuse)) * light).min(Vec3::ONE)
    }
}

/// The lit colour of every vertex of a layer, row by row.
pub fn light_layer(map: &TerrainMap, layer: &TerrainLayer, lights: &VertexLights) -> Vec<Vec3> {
    let mut colors = Vec::with_capacity((map.width + 1) * (map.depth + 1));
    for row in 0..=map.depth {
        for col in 0..=map.width {
            let normal = map.vertex_normal(layer, row, col);
            let diffuse = layer.vertex_colors[row * (map.width + 1) + col];
            colors.push(lights.light(diffuse, normal));
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darkening_matches_the_original_rounding() {
        assert_eq!(darken_rgb565(0xffff), (21 << 11) | (44 << 5) | 21);
        assert_eq!(darken_rgb565(0), 0);
    }

    #[test]
    fn a_flat_floor_gets_ambient_plus_the_main_light() {
        let settings = crate::level::LevelType::Lawn.settings();
        let lights = VertexLights::new(settings);
        let [dir0, dir1] = settings.fill_directions();
        let expected = Vec3::new(1.0, 1.0, 0.9) * 0.2
            + Vec3::new(1.0, 1.0, 0.6) * 1.1 * -dir0.y
            + Vec3::ONE * 0.5 * -dir1.y;
        let lit = lights.light(0xffff, Vec3::Y);
        let diffuse = Vec3::new(31.0 / 32.0, 63.0 / 64.0, 31.0 / 32.0);
        assert!(lit.abs_diff_eq((diffuse * expected).min(Vec3::ONE), 1e-6));
    }
}
