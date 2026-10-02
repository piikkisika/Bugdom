//! Terrain files (`Data/Terrain/*.ter.rsrc`): tile images, floor and ceiling
//! tile maps, height and vertex-colour grids, map items, splines and fences.
//!
//! Port of `ReadDataFromPlayfieldFile` (original/src/System/File.c), with the
//! structs from original/src/System/File.c, original/src/Headers/structs.h,
//! original/src/Headers/fences.h and original/src/Headers/terrain.h, and the
//! byte-swapping formats from original/src/Headers/structformats.h. How the
//! data is interpreted comes from original/src/Terrain/Terrain.c (tile bits,
//! tile image format, vertex colours, split modes), Terrain2.c (items),
//! SplineItems.c and Fences.c.
//!
//! # Format
//!
//! A terrain is a resource fork. All values are big-endian. Grids are stored
//! row by row (`[row][col]`), where a row runs along x and successive rows
//! advance along z. The map is `width` tiles along x and `depth` tiles along z.
//!
//! | Resource          | Contents |
//! |-------------------|----------|
//! | `Hedr` 1000       | 44-byte header (`PlayfieldHeaderType`, format `>I5i3f2i`): version (`NumVersion`), item count, width, depth, tile page count, tile image count, tile size, min and max height, spline count, fence count. |
//! | `Timg` 1000       | Tile images: `numTilesInList` images of 32×32 `u16` pixels, xRGB 1-5-5-5 (the original uploads them as `GL_UNSIGNED_SHORT_1_5_5_5_REV` / `GL_BGRA` into a `GL_RGB` texture, so the top bit is ignored). Row 0 is the first texture row. |
//! | `Xlat` 1000       | Translation table, `i16` per editor tile number, giving the image index in `Timg`. |
//! | `Layr` 1000, 1001 | Floor and ceiling tile maps, `depth × width` `u16`: bits 0–11 editor tile number (translated through `Xlat` on load), bits 12–13 rotation, bit 14 flip Y, bit 15 flip X. `Layr` 1002 exists in every file but is never read. |
//! | `YCrd` 1000, 1001 | Floor and ceiling heights, `(depth+1) × (width+1)` `f32` per vertex, in file units (the game multiplies by `TERRAIN_POLYGON_SIZE / tileSize`). |
//! | `Vcol` 1000, 1001 | Vertex colours, `(depth+1) × (width+1)` `u16` RGB 5-6-5. |
//! | `Splt` 1000, 1001 | Split mode per tile, one byte each ([`SplitMode`]). The game overwrites these on load (`CalculateSplitModeMatrix`); see [`SplitMode::from_corner_heights`]. |
//! | `Itms` 1000       | Items, 12 bytes each (`TerrainItemEntryType`, `>3H4bH`): x, z, type, 4 parameter bytes, flags. Sorted by x. |
//! | `Spln` 1000       | Splines, 32 bytes each (`File_SplineDefType`, `>hxxiiihxxi4h`): nub count, pointer, point count, pointer, item count, pointer, bounding `Rect`. Optional. |
//! | `SpNb`/`SpPt`/`SpIt` 1000+n | Spline n's nubs (`f32` x, z), baked points (`f32` x, z) and items (12 bytes, `>fH4bH`: placement, type, 4 parameter bytes, flags). Not read for splines with fewer than two nubs. |
//! | `Fenc` 1000       | Fences, 16 bytes each (`File_FenceDefType`, `>Hhi4h`): type, nub count, pointer, bounding `Rect`. Optional. |
//! | `FnNb` 1000+n     | Fence n's nubs (`i32` x, z). |
//!
//! Item, spline and fence coordinates are in map pixels: 32 per tile, the
//! game multiplies them by `MAP2UNIT_VALUE` (160 / 32). `Rect`s are `i16`
//! top, left, bottom, right in the same units. The files also hold `alis`,
//! `Atrb` and `ItCo` resources that the game never reads; they are ignored.
//!
//! This parser keeps the original units and does none of the load-time
//! processing other than the `Xlat` translation: no scaling, no spline loop
//! patching (`PatchSplineLoop`), no rounding of the map size down to whole
//! super-tiles (all shipped maps are already multiples of 5 tiles).

use std::io::Cursor;
use std::path::Path;

use binrw::{BinRead, BinReaderExt};

use crate::error::{Error, Result, ResultExt};
use crate::four_cc::FourCC;
use crate::rsrc::{Resource, ResourceFork};

/// Width and height in pixels of one tile image (`OREOMAP_TILE_SIZE`).
pub const TILE_IMAGE_SIZE: usize = 32;
/// Bytes per tile image in the decoded RGBA8 form.
pub const TILE_IMAGE_RGBA_LEN: usize = TILE_IMAGE_SIZE * TILE_IMAGE_SIZE * 4;
/// Map pixels per tile: item, spline and fence coordinates use this unit.
pub const MAP_PIXELS_PER_TILE: u32 = 32;

/// Resource ID of the first (floor) layer; the ceiling is the next ID.
const BASE_ID: i16 = 1000;
const TILENUM_MASK: u16 = 0x0fff;
const TILE_FLIPX_MASK: u16 = 1 << 15;
const TILE_FLIPY_MASK: u16 = 1 << 14;
const TILE_ROTATE_SHIFT: u16 = 12;
/// Bytes per tile image as stored: 16-bit pixels.
const TILE_IMAGE_STORED_LEN: usize = TILE_IMAGE_SIZE * TILE_IMAGE_SIZE * 2;

/// A whole terrain file.
#[derive(Debug, Clone)]
pub struct Terrain {
    pub header: Header,
    /// One image per tile, indexed by [`Tile::image`].
    pub tile_images: Vec<TileImage>,
    pub floor: Layer,
    /// Present in every shipped file, but the game only uses it on levels with
    /// a ceiling (`gLevelHasCeiling` in original/src/System/Main.c: the hive
    /// and the ant hill).
    pub ceiling: Option<Layer>,
    pub items: Vec<Item>,
    pub splines: Vec<Spline>,
    pub fences: Vec<Fence>,
}

/// The parts of `PlayfieldHeaderType` not implied by the lengths of the
/// parsed lists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    /// `NumVersion`: major, minor/bug, stage, non-release revision.
    pub version: [u8; 4],
    /// Map size in tiles along x (`mapWidth`).
    pub width: usize,
    /// Map size in tiles along z (`mapHeight`).
    pub depth: usize,
    /// Number of tile pages in the editor; unused by the game.
    pub tile_pages: i32,
    /// Size of a tile in the file's height units (`tileSize`).
    pub tile_size: f32,
    /// Height range in file units (`minY`, `maxY`). Unused by the game and
    /// not exact: some heights in AntHill fall outside it.
    pub min_y: f32,
    pub max_y: f32,
}

/// One 32×32 tile image, decoded to RGBA8 (alpha is always opaque).
#[derive(Clone, PartialEq, Eq)]
pub struct TileImage {
    /// Row-major RGBA8, [`TILE_IMAGE_RGBA_LEN`] bytes, row 0 first.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for TileImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileImage").finish_non_exhaustive()
    }
}

/// The floor or the ceiling.
#[derive(Debug, Clone)]
pub struct Layer {
    /// `depth × width` tiles.
    pub tiles: Grid<Tile>,
    /// `(depth+1) × (width+1)` vertex heights, in file units.
    pub heights: Grid<f32>,
    /// `(depth+1) × (width+1)` vertex colours, RGB 5-6-5; see [`rgb565_to_rgb`].
    pub vertex_colors: Grid<u16>,
    /// `depth × width` split modes as stored. The game ignores these and
    /// recomputes them from the heights.
    pub split_modes: Grid<SplitMode>,
}

/// A row-major 2D array, indexed `[row][col]` like the original's
/// `Alloc_2d_array` matrices.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid<T> {
    width: usize,
    depth: usize,
    cells: Vec<T>,
}

impl<T> Grid<T> {
    fn new(width: usize, depth: usize, cells: Vec<T>) -> Result<Self> {
        if width.checked_mul(depth) != Some(cells.len()) {
            return Err(Error::invalid(format!(
                "{} cells do not fill a {width}×{depth} grid",
                cells.len()
            )));
        }
        Ok(Self {
            width,
            depth,
            cells,
        })
    }

    /// Number of columns (along x).
    pub fn width(&self) -> usize {
        self.width
    }

    /// Number of rows (along z).
    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn get(&self, row: usize, col: usize) -> Option<&T> {
        if col < self.width {
            self.cells.get(row.checked_mul(self.width)? + col)
        } else {
            None
        }
    }

    pub fn row(&self, row: usize) -> Option<&[T]> {
        let start = row.checked_mul(self.width)?;
        self.cells
            .get(start..start.checked_add(self.width)?)
            .filter(|_| row < self.depth)
    }

    /// All cells, row by row.
    pub fn as_slice(&self) -> &[T] {
        &self.cells
    }
}

/// One map cell's tile: which image and how it is oriented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    /// Index into [`Terrain::tile_images`], already translated through `Xlat`.
    /// The original draws image 0 if this is out of range
    /// (`DrawTileIntoMipmap`); none of the shipped maps need that.
    pub image: u16,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Applied after the flips.
    pub rotation: Rotation,
}

/// Clockwise rotation in image space (row 0 at the top), in quarter turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    #[default]
    None = 0,
    Cw90 = 1,
    Cw180 = 2,
    Cw270 = 3,
}

impl Tile {
    /// Decodes a stored tile word whose tile number has already been
    /// translated to an image index.
    fn from_bits(bits: u16) -> Self {
        let rotation = match (bits >> TILE_ROTATE_SHIFT) & 3 {
            0 => Rotation::None,
            1 => Rotation::Cw90,
            2 => Rotation::Cw180,
            _ => Rotation::Cw270,
        };
        Self {
            image: bits & TILENUM_MASK,
            flip_x: bits & TILE_FLIPX_MASK != 0,
            flip_y: bits & TILE_FLIPY_MASK != 0,
            rotation,
        }
    }

    /// The pixel of the tile image that lands at `(x, y)` of the drawn tile,
    /// for an image `size` pixels square. `x` is the column and `y` the row,
    /// both in `0..size`.
    ///
    /// Port of the flip and rotation cases of `DrawTileIntoMipmap`
    /// (original/src/Terrain/Terrain.c).
    pub fn source_pixel(&self, x: usize, y: usize, size: usize) -> (usize, usize) {
        let last = size.saturating_sub(1);
        // Undo the rotation, then the flips.
        let (mut sx, mut sy) = match self.rotation {
            Rotation::None => (x, y),
            Rotation::Cw90 => (y, last - x),
            Rotation::Cw180 => (last - x, last - y),
            Rotation::Cw270 => (last - y, x),
        };
        if self.flip_x {
            sx = last - sx;
        }
        if self.flip_y {
            sy = last - sy;
        }
        (sx, sy)
    }
}

/// How a tile's quad is split into two triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SplitMode {
    /// `\`: the diagonal runs from (row, col) to (row+1, col+1).
    #[default]
    Backward = 0,
    /// `/`: the diagonal runs from (row, col+1) to (row+1, col).
    Forward = 1,
    /// Defined by the original (`SPLIT_ARBITRARY`) but never produced.
    Arbitrary = 2,
}

impl SplitMode {
    /// The split the game actually uses, from the heights of a tile's corners:
    /// `y0` at (row, col), `y1` at (row, col+1), `y2` at (row+1, col+1) and
    /// `y3` at (row+1, col). Splits along the diagonal with the smaller height
    /// difference.
    ///
    /// Port of `CalculateSplitModeMatrix` (original/src/Terrain/Terrain.c).
    pub fn from_corner_heights(y0: f32, y1: f32, y2: f32, y3: f32) -> Self {
        if (y0 == y1 && y0 == y2 && y0 == y3) || (y0 - y2).abs() < (y1 - y3).abs() {
            Self::Backward
        } else {
            Self::Forward
        }
    }
}

impl TryFrom<u8> for SplitMode {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Backward),
            1 => Ok(Self::Forward),
            2 => Ok(Self::Arbitrary),
            _ => Err(Error::invalid(format!("unknown split mode {value}"))),
        }
    }
}

/// Converts a vertex colour to RGB in `0.0..1.0`, dividing by 32, 64
/// and 32 like the original (so full intensity is slightly below 1.0).
///
/// Port of the vertex colour decoding in `BuildTerrainSuperTile`
/// (original/src/Terrain/Terrain.c).
pub fn rgb565_to_rgb(color: u16) -> [f32; 3] {
    [
        f32::from(color >> 11) / 32.0,
        f32::from((color >> 5) & 0x3f) / 64.0,
        f32::from(color & 0x1f) / 32.0,
    ]
}

/// A map item (`TerrainItemEntryType`). The meaning of `params` and `flags`
/// depends on `kind` and is left to the gameplay code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct Item {
    /// Position in map pixels along x.
    pub x: u16,
    /// Position in map pixels along z (the original calls it `y`).
    pub z: u16,
    /// Item type: index into `gTerrainItemAddRoutines` (Terrain2.c).
    pub kind: u16,
    pub params: [u8; 4],
    pub flags: u16,
}

/// A spline: a path for moving items such as enemies.
#[derive(Debug, Clone, PartialEq)]
pub struct Spline {
    /// The control points.
    pub nubs: Vec<SplinePoint>,
    /// The points baked by the editor, which the game follows.
    pub points: Vec<SplinePoint>,
    pub items: Vec<SplineItem>,
    pub bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, BinRead)]
#[br(big)]
pub struct SplinePoint {
    pub x: f32,
    pub z: f32,
}

/// An item on a spline (`SplineItemType`).
#[derive(Debug, Clone, Copy, PartialEq, BinRead)]
#[br(big)]
pub struct SplineItem {
    /// Where on the spline the item starts: 0 is the first point, 1 the end.
    pub placement: f32,
    /// Item type: index into `gSplineItemPrimeRoutines` (SplineItems.c).
    pub kind: u16,
    pub params: [u8; 4],
    pub flags: u16,
}

/// A fence: a wall along a polyline of nubs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// Fence type (selects the texture and height in Fences.c).
    pub kind: u16,
    pub nubs: Vec<FencePoint>,
    pub bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BinRead)]
#[br(big)]
pub struct FencePoint {
    pub x: i32,
    pub z: i32,
}

/// A Mac `Rect`, here in map pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, BinRead)]
#[br(big)]
pub struct Rect {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}

/// Reads a terrain file such as `Lawn.ter.rsrc`.
pub fn open(path: impl AsRef<Path>) -> Result<Terrain> {
    let path = path.as_ref();
    let fork = ResourceFork::open(path)?;
    parse(&fork).context(|| format!("in {}", path.display()))
}

/// Parses a terrain from its resource fork.
///
/// Port of `ReadDataFromPlayfieldFile` (original/src/System/File.c).
pub fn parse(fork: &ResourceFork) -> Result<Terrain> {
    let raw: RawHeader = read_one(fork, b"Hedr", BASE_ID, RAW_HEADER_LEN)?;
    let header = Header {
        version: raw.version,
        width: count(raw.map_width, "map width")?,
        depth: count(raw.map_height, "map height")?,
        tile_pages: raw.num_tile_pages,
        tile_size: raw.tile_size,
        min_y: raw.min_y,
        max_y: raw.max_y,
    };

    let num_tiles = count(raw.num_tiles_in_list, "tile count")?;
    let tile_images = tile_images(require(fork, b"Timg", BASE_ID)?, num_tiles)
        .context(|| "resource 'Timg' 1000".into())?;

    let xlat_res = require(fork, b"Xlat", BASE_ID)?;
    let xlat = xlat_table(&xlat_res.data).context(|| "resource 'Xlat' 1000".into())?;

    let floor = layer(fork, &header, &xlat, 0)?;
    let ceiling = if fork.get(FourCC::new(b"Layr"), BASE_ID + 1).is_some() {
        Some(layer(fork, &header, &xlat, 1)?)
    } else {
        None
    };

    let num_items = count(raw.num_items, "item count")?;
    let items = read_records(fork, b"Itms", BASE_ID, num_items, ITEM_LEN)?;

    let splines = splines(fork, count(raw.num_splines, "spline count")?)?;
    let fences = fences(fork, count(raw.num_fences, "fence count")?)?;

    Ok(Terrain {
        header,
        tile_images,
        floor,
        ceiling,
        items,
        splines,
        fences,
    })
}

/// `PlayfieldHeaderType` as stored.
#[derive(BinRead)]
#[br(big)]
struct RawHeader {
    version: [u8; 4],
    num_items: i32,
    map_width: i32,
    map_height: i32,
    num_tile_pages: i32,
    num_tiles_in_list: i32,
    tile_size: f32,
    min_y: f32,
    max_y: f32,
    num_splines: i32,
    num_fences: i32,
}

const RAW_HEADER_LEN: usize = 44;
const ITEM_LEN: usize = 12;
const SPLINE_POINT_LEN: usize = 8;
const SPLINE_ITEM_LEN: usize = 12;
const RAW_SPLINE_LEN: usize = 32;
const RAW_FENCE_LEN: usize = 16;
const FENCE_POINT_LEN: usize = 8;

/// `File_SplineDefType` as stored. The pointers are meaningless on disk.
#[derive(BinRead)]
#[br(big)]
struct RawSpline {
    num_nubs: i16,
    _pad1: i16,
    _junk1: i32,
    num_points: i32,
    _junk2: i32,
    num_items: i16,
    _pad2: i16,
    _junk3: i32,
    bounds: Rect,
}

/// `File_FenceDefType` as stored.
#[derive(BinRead)]
#[br(big)]
struct RawFence {
    kind: u16,
    num_nubs: i16,
    _junk: i32,
    bounds: Rect,
}

fn require<'a>(fork: &'a ResourceFork, kind: &[u8; 4], id: i16) -> Result<&'a Resource> {
    fork.require(FourCC::new(kind), id)
}

/// Converts a count from the file, rejecting negative values.
fn count(value: impl TryInto<usize> + Copy + std::fmt::Display, what: &str) -> Result<usize> {
    value
        .try_into()
        .map_err(|_| Error::invalid(format!("invalid {what} {value}")))
}

/// Checks that `data` holds exactly `count` records of `size` bytes, as
/// `CHECK_HANDLE_SIZE_BEFORE_UNPACK` does.
fn check_len(data: &[u8], count: usize, size: usize) -> Result<()> {
    let expected = count.checked_mul(size);
    if expected == Some(data.len()) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "expected {count} × {size} bytes, found {}",
            data.len()
        )))
    }
}

fn read_one<T>(fork: &ResourceFork, kind: &[u8; 4], id: i16, size: usize) -> Result<T>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    let records: Vec<T> = read_records(fork, kind, id, 1, size)?;
    records
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("empty record list"))
}

/// Reads `count` big-endian records of `size` bytes from a required resource.
fn read_records<T>(
    fork: &ResourceFork,
    kind: &[u8; 4],
    id: i16,
    count: usize,
    size: usize,
) -> Result<Vec<T>>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    let resource = require(fork, kind, id)?;
    parse_records(&resource.data, count, size)
        .context(|| format!("resource {:?} {id}", FourCC::new(kind)))
}

fn parse_records<T>(data: &[u8], count: usize, size: usize) -> Result<Vec<T>>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    check_len(data, count, size)?;
    let mut reader = Cursor::new(data);
    (0..count)
        .map(|_| reader.read_be::<T>().map_err(Error::from))
        .collect()
}

/// Reads a grid of big-endian scalars of `N` bytes each.
fn scalar_grid<T, const N: usize>(
    fork: &ResourceFork,
    kind: &[u8; 4],
    id: i16,
    width: usize,
    depth: usize,
    decode: impl Fn([u8; N]) -> Result<T>,
) -> Result<Grid<T>> {
    let resource = require(fork, kind, id)?;
    let parse = || {
        let cells = width
            .checked_mul(depth)
            .ok_or_else(|| Error::invalid("grid too large"))?;
        check_len(&resource.data, cells, N)?;
        let (chunks, _) = resource.data.as_chunks::<N>();
        let values = chunks.iter().map(|c| decode(*c)).collect::<Result<_>>()?;
        Grid::new(width, depth, values)
    };
    parse().context(|| format!("resource {:?} {id}", FourCC::new(kind)))
}

/// Decodes the xRGB 1-5-5-5 tile images to RGBA8.
fn tile_images(resource: &Resource, num_tiles: usize) -> Result<Vec<TileImage>> {
    check_len(&resource.data, num_tiles, TILE_IMAGE_STORED_LEN)?;
    // OpenGL normalises each 5-bit channel to v / 31 and the 8-bit texture
    // stores round(v / 31 * 255).
    let expand = |v: u16| ((u32::from(v & 0x1f) * 255 + 15) / 31) as u8;
    Ok(resource
        .data
        .chunks_exact(TILE_IMAGE_STORED_LEN)
        .map(|tile| {
            let (pixels, _) = tile.as_chunks::<2>();
            let rgba = pixels
                .iter()
                .flat_map(|&p| {
                    let p = u16::from_be_bytes(p);
                    [expand(p >> 10), expand(p >> 5), expand(p), u8::MAX]
                })
                .collect();
            TileImage { rgba }
        })
        .collect())
}

fn xlat_table(data: &[u8]) -> Result<Vec<u16>> {
    // `UNPACK_BE_SCALARS_AUTOSIZEHANDLE`: the length comes from the resource.
    let (entries, rest) = data.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(Error::invalid("odd length"));
    }
    entries
        .iter()
        .map(|&e| {
            let image = i16::from_be_bytes(e);
            // The image number is OR-ed into the tile word, so anything wider
            // than the tile number field would corrupt the flags.
            u16::try_from(image)
                .ok()
                .filter(|&i| i <= TILENUM_MASK)
                .ok_or_else(|| Error::invalid(format!("invalid image index {image}")))
        })
        .collect()
}

/// Reads one layer: index 0 is the floor, 1 the ceiling.
fn layer(fork: &ResourceFork, header: &Header, xlat: &[u16], index: i16) -> Result<Layer> {
    let id = BASE_ID + index;
    let (w, d) = (header.width, header.depth);
    let (vw, vd) = (w + 1, d + 1);

    let tiles = scalar_grid(fork, b"Layr", id, w, d, |b| {
        let bits = u16::from_be_bytes(b);
        let number = bits & TILENUM_MASK;
        let image = xlat
            .get(usize::from(number))
            .ok_or_else(|| Error::invalid(format!("tile {number} is not in 'Xlat'")))?;
        Ok(Tile::from_bits((bits & !TILENUM_MASK) | image))
    })?;
    let heights = scalar_grid(fork, b"YCrd", id, vw, vd, |b| Ok(f32::from_be_bytes(b)))?;
    let vertex_colors = scalar_grid(fork, b"Vcol", id, vw, vd, |b| Ok(u16::from_be_bytes(b)))?;
    let split_modes = scalar_grid(fork, b"Splt", id, w, d, |[b]| SplitMode::try_from(b))?;

    Ok(Layer {
        tiles,
        heights,
        vertex_colors,
        split_modes,
    })
}

fn splines(fork: &ResourceFork, num_splines: usize) -> Result<Vec<Spline>> {
    // A missing 'Spln' means no splines, whatever the header says.
    if fork.get(FourCC::new(b"Spln"), BASE_ID).is_none() {
        return Ok(Vec::new());
    }
    let raw: Vec<RawSpline> = read_records(fork, b"Spln", BASE_ID, num_splines, RAW_SPLINE_LEN)?;
    raw.iter()
        .enumerate()
        .map(|(i, s)| spline(fork, i, s).context(|| format!("spline {i}")))
        .collect()
}

fn spline(fork: &ResourceFork, index: usize, raw: &RawSpline) -> Result<Spline> {
    let num_nubs = count(raw.num_nubs, "nub count")?;
    let num_points = count(raw.num_points, "point count")?;
    let num_items = count(raw.num_items, "item count")?;

    // Lawn's spline 16 has one nub and no points or items; the original skips
    // its resources (which hold one stray record each).
    if num_nubs < 2 {
        if num_points != 0 || num_items != 0 {
            return Err(Error::invalid("spline without nubs has points or items"));
        }
        return Ok(Spline {
            nubs: Vec::new(),
            points: Vec::new(),
            items: Vec::new(),
            bounds: raw.bounds,
        });
    }
    if num_points < 2 || num_items < 1 {
        return Err(Error::invalid(format!(
            "spline has {num_points} points and {num_items} items"
        )));
    }

    let id = resource_id(index)?;
    Ok(Spline {
        nubs: read_records(fork, b"SpNb", id, num_nubs, SPLINE_POINT_LEN)?,
        points: read_records(fork, b"SpPt", id, num_points, SPLINE_POINT_LEN)?,
        items: read_records(fork, b"SpIt", id, num_items, SPLINE_ITEM_LEN)?,
        bounds: raw.bounds,
    })
}

fn fences(fork: &ResourceFork, num_fences: usize) -> Result<Vec<Fence>> {
    if fork.get(FourCC::new(b"Fenc"), BASE_ID).is_none() {
        return Ok(Vec::new());
    }
    let raw: Vec<RawFence> = read_records(fork, b"Fenc", BASE_ID, num_fences, RAW_FENCE_LEN)?;
    raw.iter()
        .enumerate()
        .map(|(i, f)| {
            let read = || -> Result<Fence> {
                let num_nubs = count(f.num_nubs, "nub count")?;
                Ok(Fence {
                    kind: f.kind,
                    nubs: read_records(fork, b"FnNb", resource_id(i)?, num_nubs, FENCE_POINT_LEN)?,
                    bounds: f.bounds,
                })
            };
            read().context(|| format!("fence {i}"))
        })
        .collect()
}

/// The resource ID of the `index`th spline or fence's data.
fn resource_id(index: usize) -> Result<i16> {
    i16::try_from(index)
        .ok()
        .and_then(|i| BASE_ID.checked_add(i))
        .ok_or_else(|| Error::invalid(format!("too many entries ({index})")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;

    const NAMES: [&str; 10] = [
        "AntHill", "AntKing", "Beach", "BeeHive", "Flight", "Lawn", "Night", "Pond", "QueenBee",
        "Training",
    ];

    fn load(name: &str) -> Terrain {
        open(original_data_dir().join(format!("Terrain/{name}.ter.rsrc")))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    fn all() -> Vec<(&'static str, Terrain)> {
        NAMES.iter().map(|&n| (n, load(n))).collect()
    }

    #[test]
    fn every_terrain_file_is_covered() {
        let mut files: Vec<_> = std::fs::read_dir(original_data_dir().join("Terrain"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".ter.rsrc"))
            .collect();
        files.sort();
        let expected: Vec<_> = NAMES.iter().map(|n| format!("{n}.ter.rsrc")).collect();
        assert_eq!(files, expected);
    }

    #[test]
    fn grids_match_header() {
        for (name, t) in all() {
            let (w, d) = (t.header.width, t.header.depth);
            // LoadPlayfield rounds the size down to whole super-tiles; check
            // that this never drops anything.
            assert_eq!((w % 5, d % 5), (0, 0), "{name}");
            assert_eq!(t.header.version, [7, 0, 0, 0], "{name}");
            let ceiling = t.ceiling.as_ref().expect("every file has a ceiling");
            for layer in [&t.floor, ceiling] {
                assert_eq!((layer.tiles.width(), layer.tiles.depth()), (w, d));
                assert_eq!(
                    (layer.split_modes.width(), layer.split_modes.depth()),
                    (w, d)
                );
                assert_eq!(
                    (layer.heights.width(), layer.heights.depth()),
                    (w + 1, d + 1)
                );
                assert_eq!(
                    (layer.vertex_colors.width(), layer.vertex_colors.depth()),
                    (w + 1, d + 1)
                );
            }
        }
    }

    #[test]
    fn known_sizes() {
        let lawn = load("Lawn");
        assert_eq!((lawn.header.width, lawn.header.depth), (160, 160));
        assert_eq!(lawn.tile_images.len(), 33);
        assert_eq!(lawn.items.len(), 996);
        assert_eq!(lawn.splines.len(), 23);
        assert_eq!(lawn.fences.len(), 40);
        assert_eq!(lawn.header.tile_size, 62.5);

        let night = load("Night");
        assert_eq!((night.header.width, night.header.depth), (250, 200));
    }

    #[test]
    fn tiles_reference_existing_images() {
        for (name, t) in all() {
            assert!(
                t.tile_images
                    .iter()
                    .all(|i| i.rgba.len() == TILE_IMAGE_RGBA_LEN)
            );
            for layer in std::iter::once(&t.floor).chain(&t.ceiling) {
                for tile in layer.tiles.as_slice() {
                    assert!(
                        usize::from(tile.image) < t.tile_images.len(),
                        "{name}: {tile:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn night_uses_flipped_and_rotated_tiles() {
        let night = load("Night");
        let tiles = night.floor.tiles.as_slice();
        assert!(tiles.iter().any(|t| t.flip_x && !t.flip_y));
        assert!(tiles.iter().any(|t| t.flip_y && !t.flip_x));
        assert!(tiles.iter().any(|t| t.rotation == Rotation::Cw90));
    }

    #[test]
    fn heights_are_finite() {
        for (name, t) in all() {
            for layer in std::iter::once(&t.floor).chain(&t.ceiling) {
                assert!(
                    layer.heights.as_slice().iter().all(|y| y.is_finite()),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn items_lie_inside_map_and_are_sorted() {
        for (name, t) in all() {
            let max_x = t.header.width as u32 * MAP_PIXELS_PER_TILE;
            let max_z = t.header.depth as u32 * MAP_PIXELS_PER_TILE;
            for item in &t.items {
                assert!(u32::from(item.x) < max_x, "{name}: {item:?}");
                assert!(u32::from(item.z) < max_z, "{name}: {item:?}");
            }
            // BuildTerrainItemList relies on the list being sorted by
            // super-tile column.
            let column = |i: &Item| i.x / (5 * MAP_PIXELS_PER_TILE as u16);
            assert!(t.items.is_sorted_by_key(column), "{name}");
            // Item type 0 is the player's start position.
            assert!(t.items.iter().any(|i| i.kind == 0), "{name}");
        }
    }

    #[test]
    fn splines_and_fences_are_consistent() {
        for (name, t) in all() {
            for (i, s) in t.splines.iter().enumerate() {
                if s.nubs.is_empty() {
                    assert!(name == "Lawn" && i == 16, "{name} spline {i} is empty");
                    continue;
                }
                assert!(s.points.len() >= s.nubs.len(), "{name} spline {i}");
                for item in &s.items {
                    assert!((0.0..=1.0).contains(&item.placement), "{name}: {item:?}");
                }
                // PatchSplineLoop requires the ends to (nearly) meet.
                let (a, b) = (s.nubs[0], s.nubs[s.nubs.len() - 1]);
                assert!((a.x - b.x).hypot(a.z - b.z) < 20.0, "{name} spline {i}");
            }
            for (i, f) in t.fences.iter().enumerate() {
                assert!(f.nubs.len() >= 2, "{name} fence {i}");
                for n in &f.nubs {
                    let b = f.bounds;
                    let inside = (i32::from(b.left)..=i32::from(b.right)).contains(&n.x)
                        && (i32::from(b.top)..=i32::from(b.bottom)).contains(&n.z);
                    assert!(inside, "{name} fence {i}: {n:?} outside {b:?}");
                }
            }
        }
        let anthill = load("AntHill");
        assert_eq!((anthill.splines.len(), anthill.fences.len()), (7, 18));
        assert_eq!(anthill.splines[0].nubs.len(), 6);
        assert_eq!(anthill.splines[0].points.len(), 1226);
        assert_eq!(anthill.splines[0].items.len(), 4);
        let king = load("AntKing");
        assert!(king.splines.is_empty() && king.fences.is_empty());
    }

    #[test]
    fn tile_images_decode_to_opaque_rgba() {
        let lawn = load("Lawn");
        for image in &lawn.tile_images {
            assert!(image.rgba.chunks_exact(4).all(|p| p[3] == 255));
        }
        // Not every image is a single colour.
        assert!(lawn.tile_images.iter().any(|i| {
            let first = &i.rgba[..4];
            i.rgba.chunks_exact(4).any(|p| p != first)
        }));
    }

    #[test]
    fn rgb555_expansion_covers_full_range() {
        // White with the ignored top bit set, black, then pure red.
        let mut data = vec![0xff, 0xff, 0x00, 0x00, 0x7c, 0x00];
        data.resize(TILE_IMAGE_STORED_LEN, 0);
        let fork = Resource {
            kind: FourCC::new(b"Timg"),
            id: 1000,
            name: None,
            data,
        };
        let images = tile_images(&fork, 1).unwrap();
        assert_eq!(
            &images[0].rgba[..12],
            &[255, 255, 255, 255, 0, 0, 0, 255, 255, 0, 0, 255]
        );
    }

    /// Direct port of the `switch` in `DrawTileIntoMipmap`, writing into a
    /// buffer exactly one tile wide.
    fn draw_tile_like_original(bits: u16, src: &[u32], size: usize) -> Vec<u32> {
        let mut dst = vec![0; size * size];
        let s = size as isize;
        let (flip_x, flip_y) = (1u16 << 15, 1u16 << 14);
        let (rot1, rot2, rot3) = (1u16 << 12, 2u16 << 12, 3u16 << 12);
        let xy = flip_x | flip_y;
        let mut put = |x: isize, y: isize, v: u32| dst[(y * s + x) as usize] = v;
        let at = |x: isize, y: isize| src[(y * s + x) as usize];
        for y in 0..s {
            for x in 0..s {
                match bits {
                    b if b == 0 || b == xy | rot2 => put(x, y, at(x, y)),
                    b if b == flip_x || b == flip_y | rot2 => put(x, y, at(s - 1 - x, y)),
                    b if b == flip_y || b == flip_x | rot2 => put(x, y, at(x, s - 1 - y)),
                    b if b == xy || b == rot2 => put(x, y, at(s - 1 - x, s - 1 - y)),
                    // Buffer starts at the right column; src row y goes down
                    // dst column s-1-y.
                    b if b == rot1 || b == xy | rot3 => put(s - 1 - y, x, at(x, y)),
                    b if b == rot3 || b == xy | rot1 => put(y, s - 1 - x, at(x, y)),
                    b if b == flip_x | rot1 || b == flip_y | rot3 => {
                        put(s - 1 - y, s - 1 - x, at(x, y))
                    }
                    b if b == flip_x | rot3 || b == flip_y | rot1 => put(y, x, at(x, y)),
                    _ => unreachable!(),
                }
            }
        }
        dst
    }

    #[test]
    fn source_pixel_matches_draw_tile_into_mipmap() {
        let size = 4;
        let src: Vec<u32> = (0..(size * size) as u32).collect();
        for flags in 0..16u16 {
            let bits = flags << 12;
            let expected = draw_tile_like_original(bits, &src, size);
            let tile = Tile::from_bits(bits);
            for y in 0..size {
                for x in 0..size {
                    let (sx, sy) = tile.source_pixel(x, y, size);
                    assert_eq!(src[sy * size + sx], expected[y * size + x], "{tile:?}");
                }
            }
        }
    }

    #[test]
    fn decodes_tile_bits() {
        let tile = Tile::from_bits(0x8000 | 0x2000 | 0x0123);
        assert_eq!(
            tile,
            Tile {
                image: 0x123,
                flip_x: true,
                flip_y: false,
                rotation: Rotation::Cw180,
            }
        );
    }

    #[test]
    fn split_mode_from_heights() {
        assert_eq!(
            SplitMode::from_corner_heights(1.0, 1.0, 1.0, 1.0),
            SplitMode::Backward
        );
        // y0-y2 differs less than y1-y3.
        assert_eq!(
            SplitMode::from_corner_heights(0.0, 5.0, 1.0, 0.0),
            SplitMode::Backward
        );
        assert_eq!(
            SplitMode::from_corner_heights(0.0, 1.0, 5.0, 0.0),
            SplitMode::Forward
        );
    }

    #[test]
    fn vertex_color_conversion() {
        assert_eq!(rgb565_to_rgb(0), [0.0, 0.0, 0.0]);
        assert_eq!(
            rgb565_to_rgb(0xffff),
            [31.0 / 32.0, 63.0 / 64.0, 31.0 / 32.0]
        );
    }
}
