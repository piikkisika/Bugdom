//! The terrain as the game uses it: heights in world units, split modes and
//! vertex colours, for the floor and (on levels that have one) the ceiling.

use bevy::prelude::*;
use bugdom_formats::terrain::{Grid, Layer, SplitMode, Terrain, Tile};

/// Size of a terrain tile in world units (`TERRAIN_POLYGON_SIZE`).
pub const TILE_SIZE: f32 = 160.0;
/// Tiles along each side of a supertile (`SUPERTILE_SIZE`): the original's
/// streaming unit, and our chunk size.
pub const SUPERTILE_TILES: usize = 5;
/// World units per map pixel, the unit of item, spline and fence coordinates
/// (`MAP2UNIT_VALUE`).
pub const MAP_TO_WORLD: f32 = 160.0 / 32.0;

/// Floor or ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerKind {
    Floor,
    Ceiling,
}

/// The current level's terrain.
#[derive(Resource, Debug, Clone)]
pub struct TerrainMap {
    /// Width in tiles (along x).
    pub width: usize,
    /// Depth in tiles (along z).
    pub depth: usize,
    pub floor: TerrainLayer,
    /// Only on levels with a ceiling (`gDoCeiling`).
    pub ceiling: Option<TerrainLayer>,
}

/// One layer of the terrain.
#[derive(Debug, Clone)]
pub struct TerrainLayer {
    pub kind: LayerKind,
    /// `depth × width` tiles.
    pub tiles: Vec<Tile>,
    /// `(depth+1) × (width+1)` vertex heights in world units (`gMapYCoords`).
    pub heights: Vec<f32>,
    /// `depth × width` split modes, recomputed from the heights
    /// (`CalculateSplitModeMatrix`).
    pub split_modes: Vec<SplitMode>,
    /// `(depth+1) × (width+1)` vertex colours, RGB 5-6-5 (`gVertexColors`).
    pub vertex_colors: Vec<u16>,
}

impl TerrainMap {
    /// Builds the map from a terrain file, as `LoadPlayfield` does
    /// (original/src/System/File.c), keeping the ceiling only if the level
    /// uses it.
    pub fn new(terrain: &Terrain, with_ceiling: bool) -> Self {
        let header = &terrain.header;
        // `LoadPlayfield` rounds the map down to whole supertiles. All the
        // shipped maps already are, but the grids still need the true width.
        let width = header.width / SUPERTILE_TILES * SUPERTILE_TILES;
        let depth = header.depth / SUPERTILE_TILES * SUPERTILE_TILES;
        let y_scale = TILE_SIZE / header.tile_size;
        let layer = |layer: &Layer, kind| TerrainLayer::new(layer, kind, width, depth, y_scale);
        Self {
            width,
            depth,
            floor: layer(&terrain.floor, LayerKind::Floor),
            ceiling: terrain
                .ceiling
                .as_ref()
                .filter(|_| with_ceiling)
                .map(|c| layer(c, LayerKind::Ceiling)),
        }
    }

    pub fn layer(&self, kind: LayerKind) -> Option<&TerrainLayer> {
        match kind {
            LayerKind::Floor => Some(&self.floor),
            LayerKind::Ceiling => self.ceiling.as_ref(),
        }
    }

    /// The height of a layer at a world position, and the normal of the
    /// triangle there (floor normals point up, ceiling normals down).
    ///
    /// Port of `GetTerrainHeightAtCoord` (original/src/Terrain/Terrain.c),
    /// which also leaves the normal in `gRecentTerrainNormal`. Outside the map
    /// the original returns 0 and leaves the previous normal in place; this
    /// returns an up vector instead. Without a ceiling, the ceiling is at
    /// [`NO_CEILING_HEIGHT`].
    pub fn height_at(&self, x: f32, z: f32, kind: LayerKind) -> (f32, Vec3) {
        let Some(layer) = self.layer(kind) else {
            return (NO_CEILING_HEIGHT, Vec3::NEG_Y);
        };
        // `as` truncates toward zero, like the C conversion, so the first
        // row and column extend slightly below zero.
        let col = (x / TILE_SIZE) as i64;
        let row = (z / TILE_SIZE) as i64;
        if col < 0 || row < 0 || col >= self.width as i64 || row >= self.depth as i64 {
            return (0.0, Vec3::Y);
        }
        let (row, col) = (row as usize, col as usize);
        let mut xi = x - col as f32 * TILE_SIZE;
        let zi = z - row as f32 * TILE_SIZE;
        let corner = |r: usize, c: usize| {
            Vec3::new(
                c as f32 * TILE_SIZE,
                self.height(layer, r, c),
                r as f32 * TILE_SIZE,
            )
        };
        let p = [
            corner(row, col),
            corner(row, col + 1),
            corner(row + 1, col + 1),
            corner(row + 1, col),
        ];
        let floor = kind == LayerKind::Floor;
        let (a, b, c) = if layer.split_modes[row * self.width + col] == SplitMode::Backward {
            match (floor, xi < zi) {
                (true, true) => (p[0], p[2], p[3]),
                (true, false) => (p[0], p[1], p[2]),
                (false, true) => (p[3], p[2], p[0]),
                (false, false) => (p[2], p[1], p[0]),
            }
        } else {
            xi = TILE_SIZE - xi;
            match (floor, xi > zi) {
                (true, true) => (p[0], p[1], p[3]),
                (true, false) => (p[1], p[2], p[3]),
                (false, true) => (p[3], p[1], p[0]),
                (false, false) => (p[3], p[2], p[1]),
            }
        };
        let plane = PlaneEquation::of_triangle(a, b, c);
        (plane.y_at(x, z), plane.normal)
    }

    /// The floor height at a world position.
    pub fn floor_height(&self, x: f32, z: f32) -> f32 {
        self.height_at(x, z, LayerKind::Floor).0
    }

    pub fn layers(&self) -> impl Iterator<Item = &TerrainLayer> {
        std::iter::once(&self.floor).chain(&self.ceiling)
    }

    /// Supertiles along x and z.
    pub fn supertiles(&self) -> (usize, usize) {
        (self.width / SUPERTILE_TILES, self.depth / SUPERTILE_TILES)
    }

    fn vertex_index(&self, row: usize, col: usize) -> usize {
        row * (self.width + 1) + col
    }

    /// Height of a vertex, in world units.
    pub fn height(&self, layer: &TerrainLayer, row: usize, col: usize) -> f32 {
        layer.heights[self.vertex_index(row, col)]
    }

    /// The face normals of a tile's two triangles, or two up vectors outside
    /// the map. Floor normals point up, ceiling normals down.
    ///
    /// Port of `CalcTileNormals` (original/src/Terrain/Terrain2.c).
    pub fn tile_normals(&self, layer: &TerrainLayer, row: isize, col: isize) -> [Vec3; 2] {
        let (Ok(row), Ok(col)) = (usize::try_from(row), usize::try_from(col)) else {
            return [Vec3::Y; 2];
        };
        if row >= self.depth || col >= self.width {
            return [Vec3::Y; 2];
        }
        let corner = |r: usize, c: usize| {
            Vec3::new(
                (c - col) as f32 * TILE_SIZE,
                self.height(layer, r, c),
                (r - row) as f32 * TILE_SIZE,
            )
        };
        let far_left = corner(row, col);
        let far_right = corner(row, col + 1);
        let near_right = corner(row + 1, col + 1);
        let near_left = corner(row + 1, col);
        let split = layer.split_modes[row * self.width + col];
        match (layer.kind, split) {
            (LayerKind::Floor, SplitMode::Backward) => [
                face_normal(far_left, near_left, near_right),
                face_normal(far_left, near_right, far_right),
            ],
            (LayerKind::Floor, _) => [
                face_normal(far_left, near_left, far_right),
                face_normal(far_right, near_left, near_right),
            ],
            (LayerKind::Ceiling, SplitMode::Backward) => [
                face_normal(near_right, near_left, far_left),
                face_normal(far_right, near_right, far_left),
            ],
            (LayerKind::Ceiling, _) => [
                face_normal(far_right, near_left, far_left),
                face_normal(near_right, near_left, far_right),
            ],
        }
    }

    /// A vertex's normal: the average of the face normals of the eight
    /// triangles around it.
    ///
    /// Port of the vertex normal loop in `BuildTerrainSuperTile`
    /// (original/src/Terrain/Terrain.c). Its in-supertile shortcut uses the
    /// same face normals, so computing every tile with `CalcTileNormals` is
    /// equivalent.
    pub fn vertex_normal(&self, layer: &TerrainLayer, row: usize, col: usize) -> Vec3 {
        let (row, col) = (row as isize, col as isize);
        let mut sum = Vec3::ZERO;
        for r in [row - 1, row] {
            for c in [col - 1, col] {
                let [a, b] = self.tile_normals(layer, r, c);
                sum += a + b;
            }
        }
        sum.normalize_or(Vec3::Y)
    }
}

impl TerrainLayer {
    fn new(layer: &Layer, kind: LayerKind, width: usize, depth: usize, y_scale: f32) -> Self {
        let tiles = crop_grid(&layer.tiles, width, depth);
        let heights: Vec<f32> = crop_grid(&layer.heights, width + 1, depth + 1)
            .into_iter()
            .map(|y| y * y_scale)
            .collect();
        let vertex_colors = crop_grid(&layer.vertex_colors, width + 1, depth + 1);
        let height = |row: usize, col: usize| heights[row * (width + 1) + col];
        let mut split_modes = Vec::with_capacity(width * depth);
        for row in 0..depth {
            for col in 0..width {
                split_modes.push(SplitMode::from_corner_heights(
                    height(row, col),
                    height(row, col + 1),
                    height(row + 1, col + 1),
                    height(row + 1, col),
                ));
            }
        }
        Self {
            kind,
            tiles,
            heights,
            split_modes,
            vertex_colors,
        }
    }
}

/// The top-left `width × depth` cells of a grid.
fn crop_grid<T: Copy>(grid: &Grid<T>, width: usize, depth: usize) -> Vec<T> {
    (0..depth)
        .filter_map(|row| grid.row(row))
        .flat_map(|cells| cells.iter().take(width).copied())
        .collect()
}

/// Ceiling height reported on levels without a ceiling.
pub const NO_CEILING_HEIGHT: f32 = 10_000_000.0;

/// A plane `normal · p = constant` (`TQ3PlaneEquation`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneEquation {
    pub normal: Vec3,
    pub constant: f32,
}

impl PlaneEquation {
    /// The plane through three points, with the arguments in the order of the
    /// original's calls.
    ///
    /// Port of `CalcPlaneEquationOfTriangle` (original/src/QD3D/3DMath.c),
    /// which names its parameters in reverse (`p3, p2, p1`).
    pub fn of_triangle(p3: Vec3, p2: Vec3, p1: Vec3) -> Self {
        let normal = (p1 - p2).cross(p1 - p3).normalize_or(Vec3::Y);
        Self {
            normal,
            constant: normal.dot(p1),
        }
    }

    /// The y of the plane above `(x, z)` (`IntersectionOfYAndPlane`).
    pub fn y_at(&self, x: f32, z: f32) -> f32 {
        (self.constant - (self.normal.x * x + self.normal.z * z)) / self.normal.y
    }
}

/// The unit normal of the triangle `p1 p2 p3`.
///
/// Port of `CalcFaceNormal` (original/src/QD3D/3DMath.c).
pub fn face_normal(p1: Vec3, p2: Vec3, p3: Vec3) -> Vec3 {
    (p1 - p3).cross(p2 - p3).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bugdom_formats::rsrc::ResourceFork;

    fn load(name: &str, with_ceiling: bool) -> TerrainMap {
        let path = bugdom_formats::original_data_dir().join(format!("Terrain/{name}.ter.rsrc"));
        let fork = ResourceFork::open(&path).expect("terrain file");
        TerrainMap::new(
            &bugdom_formats::terrain::parse(&fork).expect("terrain"),
            with_ceiling,
        )
    }

    #[test]
    fn heights_match_the_vertices_and_interpolate_on_the_split() {
        for (name, ceiling) in [("Lawn", false), ("BeeHive", true), ("AntKing", true)] {
            let map = load(name, ceiling);
            for layer in map.layers() {
                for row in (0..map.depth).step_by(7) {
                    for col in (0..map.width).step_by(5) {
                        let x = col as f32 * TILE_SIZE;
                        let z = row as f32 * TILE_SIZE;
                        // A corner inside the tile, nudged off the edges.
                        let (y, normal) = map.height_at(x + 0.01, z + 0.01, layer.kind);
                        let expected = map.height(layer, row, col);
                        assert!(
                            (y - expected).abs() < 1.0,
                            "{name} {row},{col}: {y} vs {expected}"
                        );
                        let up = normal.y > 0.0;
                        assert_eq!(up, layer.kind == LayerKind::Floor, "{name} {row},{col}");

                        // The centre lies on the split diagonal, so both
                        // triangles agree there.
                        let centre = map.height_at(x + 80.0, z + 80.0, layer.kind).0;
                        let (a, b) = match layer.split_modes[row * map.width + col] {
                            SplitMode::Backward => ((row, col), (row + 1, col + 1)),
                            _ => ((row, col + 1), (row + 1, col)),
                        };
                        let mid = (map.height(layer, a.0, a.1) + map.height(layer, b.0, b.1)) / 2.0;
                        assert!(
                            (centre - mid).abs() < 0.5,
                            "{name} {row},{col}: {centre} vs {mid}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn outside_the_map_and_without_a_ceiling() {
        let map = load("Lawn", false);
        assert_eq!(
            map.height_at(-200.0, 50.0, LayerKind::Floor),
            (0.0, Vec3::Y)
        );
        assert_eq!(
            map.height_at(500.0, 500.0, LayerKind::Ceiling).0,
            NO_CEILING_HEIGHT
        );
    }
}
