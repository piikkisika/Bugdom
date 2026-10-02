//! QuickDraw 3D metafiles (`.3dmf`, binary): the models in `Data/Models` and
//! the skinned reference models in `Data/Skeletons`.
//!
//! Port of `Q3MetaFileParser` (original/extern/Pomme/src/QD3D/3DMFParser.cpp,
//! 3DMFInternal.h), `Q3Pixmap_ApplyEdgePadding` and `Q3TriMeshData_New`
//! (original/extern/Pomme/src/QD3D/QD3D.cpp), plus the texture-format switch of
//! `Render_Load3DMFTextures` (original/src/QD3D/Renderer.c) and the object
//! bounds of `QD3D_CalcObjectBoundingBox`/`QD3D_CalcObjectBoundingSphere`
//! (original/src/QD3D/QD3D_Geometry.c).
//!
//! # Format
//!
//! All numbers are big-endian; floats are IEEE 754 single precision.
//!
//! The file starts with a 24-byte header: magic `3DMF`, header length (16),
//! version major/minor (u16 each, 1.5 or 1.6), flags (u32, 0 = normal file,
//! no database/stream mode) and the absolute offset of a table of contents
//! (u64, 0 if none).
//!
//! The rest of the file is a sequence of chunks, each `type: FourCC`,
//! `size: u32`, then `size` bytes of body. There is no padding between chunks.
//! The chunk types Bugdom's files use:
//!
//! | Type   | Body |
//! |--------|------|
//! | `cntr` | Container: its body is a sequence of child chunks. |
//! | `bgng` | Begin group. The body (display group state) is skipped; the group's children are the *following sibling* chunks up to the matching `endg`. |
//! | `endg` | End group, empty. |
//! | `tmsh` | Triangle mesh: six u32 counts (triangles, triangle attributes, edges, edge attributes, points, point attributes), then the triangles as three point indices each (u8 if there are at most 0xFF points, u16 if at most 0xFFFF, else u32), the points (3 × f32), and the bounding box (min, max as 3 × f32 each, then an u32 "is empty" flag). Edges are not supported. |
//! | `atar` | Attribute array for the preceding `tmsh`: attribute type, a zero u32, position of array (0 = per triangle, 1 = per edge, 2 = per point), position in array, use flag; then one value per triangle or point. |
//! | `attr` | Attribute set marker, empty; its attributes are its siblings in the enclosing `cntr`. |
//! | `kdif` | Diffuse colour, RGB 3 × f32. |
//! | `kxpr` | Transparency colour, RGB 3 × f32 with equal components; used as the opacity. |
//! | `txsu` | Texture shader, empty; followed in its container by the image and optionally `shdr`. |
//! | `txmm` | Mipmap texture: use-mipmapping flag, pixel type, bit order, byte order, width, height, row bytes, offset (u32 each), then `row bytes × height` bytes of pixels, padded to a multiple of 4. |
//! | `txpm` | Pixmap texture: width, height, row bytes, pixel size, pixel type, bit order, byte order (u32 each), then pixels as for `txmm`. |
//! | `shdr` | UV boundary modes for the current texture: U, V (u32 each; 0 = wrap, 1 = clamp). |
//! | `rfrn` | Reference: a u32 ID looked up in the table of contents. The chunk at that offset is parsed again in place. |
//! | `toc ` | Table of contents: next TOC offset (u64), ref seed, type seed, entry type (1), entry size (16), entry count (u32 each), then entries of ref ID (u32), absolute offset (u64) and object type (FourCC). |
//!
//! A chunk type of zero is treated as the end of the data, as Pomme does.
//!
//! # How the parse result is built
//!
//! This mirrors Pomme's flattened representation, which is what the game
//! consumes, rather than QuickDraw 3D's object tree:
//!
//! - Every `tmsh` becomes a [`TriMesh`] in [`MetaFile::meshes`], in parse
//!   order. Skeleton code walks this list (`LoadBonesReferenceModel` in
//!   original/src/Skeleton/Bones.c) and refers to meshes by that index.
//! - Every `cntr` or `bgng` at nesting depth 1 (that is, a child of the
//!   file's outermost container or group) starts a new [`Group`]. A mesh is
//!   added to the most recent group, and one is created if there is none yet.
//!   `LoadGrouped3DMF` (original/src/QD3D/3DMF.c) uses the group index as the
//!   object type, i.e. `gObjectGroupList[groupNum][objectType]`.
//! - Attribute arrays, colours and texture shaders apply to the "current
//!   mesh", which is the last `tmsh` parsed and is cleared whenever any
//!   container or group ends.
//! - Each distinct `txsu` chunk (by file offset) becomes one [`Texture`];
//!   when a reference re-parses a `txsu`, the mesh gets the existing texture.
//!
//! # Ignored or unsupported
//!
//! Like Pomme, the parser rejects: other chunk types, database/stream
//! files, TOC entry types other than 1, edges and edge attributes,
//! mipmapped textures or non-zero texture offsets, little-endian bit order,
//! pixel types other than RGB32, ARGB32, RGB16 and ARGB16, per-edge
//! attributes, and attribute types other than normals, UVs and diffuse
//! colour. Per-triangle normals are skipped. Further TOCs chained through
//! "next TOC" are ignored, as is the content of `bgng` chunks (display group
//! state). Like the game (which asserts in `Render_Load3DMFTextures`), a
//! `txsu` that never gets an image is rejected.
//!
//! We are stricter than Pomme where Pomme would go on reading garbage: a leaf
//! chunk must be consumed exactly, a child may not run past its container,
//! texture byte order and UV boundary values must be 0 or 1, and nesting
//! (including chains of references) is limited.
//!
//! Pomme quirk kept on purpose: `txmm`, `txpm` and `shdr` apply to the most
//! recently *created* texture, not to the `txsu` they follow. When a
//! reference re-parses an older texture's container, the image is skipped
//! (the latest texture already has one) and a `shdr` is applied to the latest
//! texture. In the shipped data such a re-applied `shdr` never changes a
//! value.
//!
//! # Coordinates
//!
//! Positions, normals and bounding boxes are in the original model space and
//! units. UVs are stored as Pomme stores them, with `v` flipped (`1 - v`)
//! so that `v = 0` is the first row of [`Pixmap::pixels`], the same
//! convention as glTF and Bevy.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use binrw::{BinRead, BinReaderExt};

use crate::error::{Error, Result, ResultExt, read_file};
use crate::four_cc::FourCC;

/// A parsed `.3dmf` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetaFile {
    /// Every mesh in the file, in parse order (Pomme's `meshes`). A mesh
    /// reached again through a reference appears again.
    pub meshes: Vec<TriMesh>,
    /// Top-level groups in file order (Pomme's `topLevelGroups`). In a model
    /// file loaded by `LoadGrouped3DMF`, the index is the object type.
    pub groups: Vec<Group>,
    /// Textures in order of first appearance; [`TriMesh::texture`] indexes
    /// into this.
    pub textures: Vec<Texture>,
}

impl MetaFile {
    /// The meshes of one group.
    pub fn group_meshes<'a>(
        &'a self,
        group: &'a Group,
    ) -> impl Iterator<Item = &'a TriMesh> + Clone {
        group.meshes.iter().filter_map(|&i| self.meshes.get(i))
    }
}

/// A top-level group: one game object made of one or more meshes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    /// Indices into [`MetaFile::meshes`], in file order.
    pub meshes: Vec<usize>,
}

/// A triangle mesh (Pomme's `TQ3TriMeshData`).
///
/// The optional per-point arrays have one entry per position when present.
#[derive(Debug, Clone, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    /// Point indices, three per triangle, all less than `positions.len()`.
    pub triangles: Vec<[u32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Texture coordinates with `v` already flipped (see the module docs).
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Per-point diffuse colour, RGB. Pomme sets the alpha to 1.
    pub colors: Option<Vec<[f32; 3]>>,
    /// The bounding box stored in the file (not recomputed).
    pub bounding_box: BoundingBox,
    /// RGBA. RGB comes from `kdif` and alpha from `kxpr`; each defaults to 1.
    pub diffuse_color: [f32; 4],
    /// Index into [`MetaFile::textures`].
    pub texture: Option<usize>,
}

impl TriMesh {
    /// Port of `Q3TriMeshData_New` with no optional features.
    fn new(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>, bounding_box: BoundingBox) -> Self {
        Self {
            positions,
            triangles,
            normals: None,
            uvs: None,
            colors: None,
            bounding_box,
            diffuse_color: [1.0; 4],
            texture: None,
        }
    }
}

/// An axis-aligned box (`TQ3BoundingBox`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub is_empty: bool,
}

impl BoundingBox {
    pub const EMPTY: Self = Self {
        min: [0.0; 3],
        max: [0.0; 3],
        is_empty: true,
    };

    /// The box around every point of some meshes.
    ///
    /// Port of `QD3D_CalcObjectBoundingBox` (original/src/QD3D/QD3D_Geometry.c),
    /// which `LoadGrouped3DMF` stores in `gObjectGroupBBoxList`.
    pub fn of_meshes<'a>(meshes: impl IntoIterator<Item = &'a TriMesh>) -> Self {
        let mut points = meshes.into_iter().flat_map(|m| m.positions.iter());
        let Some(&first) = points.next() else {
            return Self::EMPTY;
        };
        let (min, max) = points.fold((first, first), |(mut min, mut max), p| {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
            (min, max)
        });
        Self {
            min,
            max,
            is_empty: false,
        }
    }
}

/// A sphere around some points (`TQ3BoundingSphere`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingSphere {
    pub origin: [f32; 3],
    pub radius: f32,
    pub is_empty: bool,
}

impl BoundingSphere {
    /// A sphere centred on the average of all points, reaching the farthest.
    ///
    /// Port of `QD3D_CalcObjectBoundingSphere`
    /// (original/src/QD3D/QD3D_Geometry.c), which `LoadGrouped3DMF` stores in
    /// `gObjectGroupRadiusList`. Sums in `f32` in the same order as the C
    /// code. With no points the C code divides by zero; we return an empty
    /// sphere at the origin instead.
    pub fn of_meshes<'a>(meshes: impl IntoIterator<Item = &'a TriMesh> + Clone) -> Self {
        let mut sum = [0.0_f32; 3];
        let mut count = 0_u32;
        for p in meshes.clone().into_iter().flat_map(|m| m.positions.iter()) {
            for axis in 0..3 {
                sum[axis] += p[axis];
            }
            count += 1;
        }
        if count == 0 {
            return Self {
                origin: [0.0; 3],
                radius: 0.0,
                is_empty: true,
            };
        }
        let origin = sum.map(|s| s / count as f32);
        let radius_squared = meshes
            .into_iter()
            .flat_map(|m| m.positions.iter())
            .map(|p| {
                (0..3)
                    .map(|a| (p[a] - origin[a]) * (p[a] - origin[a]))
                    .sum::<f32>()
            })
            .fold(0.0_f32, f32::max);
        Self {
            origin,
            radius: radius_squared.sqrt(),
            is_empty: false,
        }
    }
}

/// A texture and how it is sampled (Pomme's `TQ3TextureShader`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub pixmap: Pixmap,
    pub boundary_u: UvBoundary,
    pub boundary_v: UvBoundary,
}

impl Texture {
    /// How meshes using this texture are blended, from its pixel type.
    ///
    /// Port of the switch in `Render_Load3DMFTextures`
    /// (original/src/QD3D/Renderer.c).
    pub fn texturing_mode(&self) -> TexturingMode {
        match self.pixmap.pixel_type {
            PixelType::Rgb32 | PixelType::Rgb16 => TexturingMode::Opaque,
            PixelType::Argb16 => TexturingMode::AlphaTest,
            PixelType::Argb32 => TexturingMode::AlphaBlend,
        }
    }
}

/// What the renderer does with a texture's alpha (Pomme's `TQ3TexturingMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TexturingMode {
    Opaque,
    /// Discard pixels whose alpha is below the renderer's threshold.
    AlphaTest,
    AlphaBlend,
}

/// What happens to UVs outside 0..1 (`TQ3ShaderUVBoundary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UvBoundary {
    #[default]
    Wrap,
    Clamp,
}

impl UvBoundary {
    fn from_raw(raw: u32) -> Result<Self> {
        match raw {
            0 => Ok(Self::Wrap),
            1 => Ok(Self::Clamp),
            _ => Err(Error::invalid(format!("unknown UV boundary mode {raw}"))),
        }
    }
}

/// The pixel formats Pomme's parser accepts (`TQ3PixelType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelType {
    /// `0x__RRGGBB`; the top byte is ignored.
    Rgb32,
    /// `0xAARRGGBB`.
    Argb32,
    /// `0b_RRRRRGGGGGBBBBB` in 16 bits; the top bit is ignored.
    Rgb16,
    /// `0bARRRRRGGGGGBBBBB` in 16 bits; 1-bit alpha.
    Argb16,
}

impl PixelType {
    fn from_raw(raw: u32) -> Result<Self> {
        match raw {
            0 => Ok(Self::Rgb32),
            1 => Ok(Self::Argb32),
            2 => Ok(Self::Rgb16),
            3 => Ok(Self::Argb16),
            _ => Err(Error::invalid(format!("unsupported pixel type {raw}"))),
        }
    }

    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb32 | Self::Argb32 => 4,
            Self::Rgb16 | Self::Argb16 => 2,
        }
    }

    /// The alpha bits, for formats that have alpha.
    fn alpha_mask(self) -> Option<u32> {
        match self {
            Self::Argb32 => Some(0xFF00_0000),
            Self::Argb16 => Some(0x8000),
            Self::Rgb32 | Self::Rgb16 => None,
        }
    }
}

/// Texture pixels in their original format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub pixel_type: PixelType,
    /// One value per pixel, rows from the top, without row padding. 16-bit
    /// formats use the low half. Byte order is already resolved, and edge
    /// padding has been applied as Pomme does when loading.
    pub pixels: Vec<u32>,
}

impl Pixmap {
    /// Converts to 8-bit RGBA, rows from the top.
    ///
    /// Formats without alpha get an alpha of 255, matching the `GL_RGB`
    /// internal format the game uploads them with. 5-bit channels are widened
    /// by bit replication.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let widen5 = |c: u32| ((c << 3) | (c >> 2)) as u8;
        let byte = |p: u32, shift: u32| (p >> shift) as u8;
        self.pixels
            .iter()
            .flat_map(|&p| match self.pixel_type {
                PixelType::Rgb32 => [byte(p, 16), byte(p, 8), byte(p, 0), 0xFF],
                PixelType::Argb32 => [byte(p, 16), byte(p, 8), byte(p, 0), byte(p, 24)],
                PixelType::Rgb16 | PixelType::Argb16 => {
                    let alpha = match self.pixel_type {
                        PixelType::Argb16 if p & 0x8000 == 0 => 0,
                        _ => 0xFF,
                    };
                    [
                        widen5((p >> 10) & 0x1F),
                        widen5((p >> 5) & 0x1F),
                        widen5(p & 0x1F),
                        alpha,
                    ]
                }
            })
            .collect()
    }
}

/// Reads and parses a `.3dmf` file.
pub fn open(path: impl AsRef<Path>) -> Result<MetaFile> {
    let path = path.as_ref();
    let bytes = read_file(path)?;
    parse(&bytes).context(|| format!("in {}", path.display()))
}

/// Parses the contents of a `.3dmf` file.
///
/// Port of `Q3MetaFileParser::Parse3DMF`.
pub fn parse(bytes: &[u8]) -> Result<MetaFile> {
    let header: FileHeader = Cursor::new(bytes)
        .read_be()
        .context(|| "3DMF header".to_owned())?;
    if header.header_len != 16 {
        return Err(Error::invalid(format!(
            "bad 3DMF header length {}",
            header.header_len
        )));
    }
    if header.version_major != 1 || !matches!(header.version_minor, 5 | 6) {
        return Err(Error::invalid(format!(
            "unsupported 3DMF version {}.{}",
            header.version_major, header.version_minor
        )));
    }
    if header.flags != 0 {
        return Err(Error::invalid(
            "3DMF database or stream files are not supported",
        ));
    }

    let toc = if header.toc_offset == 0 {
        HashMap::new()
    } else {
        read_toc(bytes, header.toc_offset)
            .context(|| format!("table of contents at {:#x}", header.toc_offset))?
    };

    let mut parser = Parser {
        data: bytes,
        pos: FILE_HEADER_SIZE,
        depth: 0,
        nesting: 0,
        current_mesh: None,
        toc,
        known_textures: HashMap::new(),
        file: MetaFile::default(),
        textures: Vec::new(),
    };
    while parser.pos != bytes.len() {
        if let Flow::EarlyEnd = parser.parse_chunk()? {
            break;
        }
    }
    parser.finish()
}

const FILE_HEADER_SIZE: usize = 24;
const CHUNK_HEADER_SIZE: usize = 8;
/// How deep containers, groups and references may nest. Real files nest
/// about four deep; the limit only stops reference cycles and stack overflow.
const MAX_NESTING: u32 = 64;
/// Passes of `Q3Pixmap_ApplyEdgePadding`, i.e. how many pixels far colour
/// bleeds into fully transparent areas.
const EDGE_PADDING_REPEAT: usize = 8;

const CNTR: FourCC = FourCC::new(b"cntr");
const BGNG: FourCC = FourCC::new(b"bgng");
const ENDG: FourCC = FourCC::new(b"endg");
const TMSH: FourCC = FourCC::new(b"tmsh");
const ATAR: FourCC = FourCC::new(b"atar");
const ATTR: FourCC = FourCC::new(b"attr");
const KDIF: FourCC = FourCC::new(b"kdif");
const KXPR: FourCC = FourCC::new(b"kxpr");
const TXSU: FourCC = FourCC::new(b"txsu");
const TXMM: FourCC = FourCC::new(b"txmm");
const TXPM: FourCC = FourCC::new(b"txpm");
const SHDR: FourCC = FourCC::new(b"shdr");
const RFRN: FourCC = FourCC::new(b"rfrn");
const TOC: FourCC = FourCC::new(b"toc ");
const END_OF_DATA: FourCC = FourCC([0; 4]);

/// Attribute types (`TQ3AttributeTypes`) that `atar` chunks may carry.
const ATTRIBUTE_SURFACE_UV: u32 = 1;
const ATTRIBUTE_SHADING_UV: u32 = 2;
const ATTRIBUTE_NORMAL: u32 = 3;
const ATTRIBUTE_DIFFUSE_COLOR: u32 = 5;
const ATTRIBUTE_TYPE_COUNT: u32 = 13;

/// `atar` position-of-array values.
const PER_TRIANGLE: u32 = 0;
const PER_POINT: u32 = 2;

/// `TQ3Endian`.
const ENDIAN_BIG: u32 = 0;
const ENDIAN_LITTLE: u32 = 1;

#[derive(BinRead)]
#[br(big, magic = b"3DMF")]
struct FileHeader {
    header_len: u32,
    version_major: u16,
    version_minor: u16,
    flags: u32,
    toc_offset: u64,
}

#[derive(BinRead)]
#[br(big, magic = b"toc ")]
struct TocHeader {
    _size: u32,
    _next_toc: u64,
    _ref_seed: u32,
    _type_seed: u32,
    entry_type: u32,
    entry_size: u32,
    entry_count: u32,
}

#[derive(BinRead)]
#[br(big)]
struct TocEntry {
    ref_id: u32,
    offset: u64,
    _object_type: FourCC,
}

const TOC_ENTRY_SIZE: usize = 16;

#[derive(BinRead)]
#[br(big)]
struct TriMeshHeader {
    triangle_count: u32,
    _triangle_attribute_count: u32,
    edge_count: u32,
    edge_attribute_count: u32,
    point_count: u32,
    _point_attribute_count: u32,
}

#[derive(BinRead)]
#[br(big)]
struct AttributeArrayHeader {
    attribute_type: u32,
    zero: u32,
    position_of_array: u32,
    position_in_array: u32,
    use_flag: u32,
}

#[derive(BinRead)]
#[br(big)]
struct MipmapHeader {
    use_mipmapping: u32,
    pixel_type: u32,
    bit_order: u32,
    byte_order: u32,
    width: u32,
    height: u32,
    row_bytes: u32,
    offset: u32,
}

#[derive(BinRead)]
#[br(big)]
struct PixmapHeader {
    width: u32,
    height: u32,
    row_bytes: u32,
    _pixel_size: u32,
    pixel_type: u32,
    bit_order: u32,
    byte_order: u32,
}

/// Port of the TOC part of `Parse3DMF`: ref ID to absolute chunk offset.
fn read_toc(bytes: &[u8], offset: u64) -> Result<HashMap<u32, u64>> {
    let mut reader = Cursor::new(bytes);
    reader.set_position(offset);
    let header: TocHeader = reader.read_be()?;
    if header.entry_type != 1 {
        return Err(Error::invalid(format!(
            "unsupported TOC entry type {}",
            header.entry_type
        )));
    }
    if header.entry_size as usize != TOC_ENTRY_SIZE {
        return Err(Error::invalid(format!(
            "bad TOC entry size {}",
            header.entry_size
        )));
    }
    // Checked up front so a corrupt count cannot cause a huge allocation.
    let available = bytes.len().saturating_sub(reader.position() as usize);
    if (header.entry_count as usize).saturating_mul(TOC_ENTRY_SIZE) > available {
        return Err(Error::invalid(format!(
            "{} TOC entries run past the end",
            header.entry_count
        )));
    }
    (0..header.entry_count)
        .map(|_| {
            let entry: TocEntry = reader.read_be()?;
            Ok((entry.ref_id, entry.offset))
        })
        .collect()
}

/// What a chunk did to the parse.
enum Flow {
    Parsed(FourCC),
    /// A zero chunk type: Pomme stops parsing the file here.
    EarlyEnd,
}

/// A texture whose image may not have been read yet.
#[derive(Default)]
struct TextureShader {
    pixmap: Option<Pixmap>,
    boundary_u: UvBoundary,
    boundary_v: UvBoundary,
}

/// Port of `Q3MetaFileParser`.
struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
    /// Pomme's `currentDepth`: how many containers and groups we are in.
    depth: u32,
    /// Recursion guard, also counting references.
    nesting: u32,
    current_mesh: Option<usize>,
    toc: HashMap<u32, u64>,
    /// `txsu` chunk offset to texture index.
    known_textures: HashMap<usize, usize>,
    file: MetaFile,
    textures: Vec<TextureShader>,
}

impl<'a> Parser<'a> {
    fn finish(self) -> Result<MetaFile> {
        let textures = self
            .textures
            .into_iter()
            .enumerate()
            .map(|(i, shader)| {
                let pixmap = shader
                    .pixmap
                    .ok_or_else(|| Error::invalid(format!("texture {i} has no image")))?;
                Ok(Texture {
                    pixmap,
                    boundary_u: shader.boundary_u,
                    boundary_v: shader.boundary_v,
                })
            })
            .collect::<Result<_>>()?;
        Ok(MetaFile {
            textures,
            ..self.file
        })
    }

    /// Borrows the next `len` bytes and advances past them.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let bytes = self
            .pos
            .checked_add(len)
            .and_then(|end| self.data.get(self.pos..end))
            .ok_or_else(|| {
                Error::invalid(format!("{len} bytes at {:#x} run past the end", self.pos))
            })?;
        self.pos += len;
        Ok(bytes)
    }

    /// Port of `Q3MetaFileParser::Parse1Chunk`.
    fn parse_chunk(&mut self) -> Result<Flow> {
        let offset = self.pos;
        let header = self
            .take(CHUNK_HEADER_SIZE)
            .context(|| format!("chunk header at {offset:#x}"))?;
        let mut reader = Cursor::new(header);
        let kind: FourCC = reader.read_be()?;
        let size: u32 = reader.read_be()?;
        let size = size as usize;

        if kind == END_OF_DATA {
            return Ok(Flow::EarlyEnd);
        }

        self.nesting += 1;
        if self.nesting > MAX_NESTING {
            return Err(Error::invalid(format!(
                "{kind:?} chunk at {offset:#x} nests too deeply (reference cycle?)"
            )));
        }
        let flow = match kind {
            CNTR => self.parse_container(size),
            BGNG => self.parse_group(size),
            RFRN => self.parse_reference(offset, size),
            _ => {
                let body = self
                    .take(size)
                    .context(|| format!("{kind:?} chunk at {offset:#x}"))?;
                self.parse_leaf(kind, offset, body)
                    .context(|| format!("{kind:?} chunk at {offset:#x}"))
                    .map(|()| Flow::Parsed(kind))
            }
        };
        self.nesting -= 1;
        flow
    }

    fn parse_container(&mut self, size: usize) -> Result<Flow> {
        let limit = self
            .pos
            .checked_add(size)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                Error::invalid(format!("container at {:#x} runs past the end", self.pos))
            })?;
        if self.depth == 1 {
            self.file.groups.push(Group::default());
        }
        self.depth += 1;
        while self.pos != limit {
            if self.pos > limit {
                return Err(Error::invalid(format!(
                    "chunk ending at {:#x} overruns its container ending at {limit:#x}",
                    self.pos
                )));
            }
            if let Flow::EarlyEnd = self.parse_chunk()? {
                return Ok(Flow::EarlyEnd);
            }
        }
        self.depth -= 1;
        self.current_mesh = None;
        Ok(Flow::Parsed(CNTR))
    }

    fn parse_group(&mut self, size: usize) -> Result<Flow> {
        if self.depth == 1 {
            self.file.groups.push(Group::default());
        }
        self.depth += 1;
        // The body holds display group state, which nothing uses.
        self.take(size)?;
        loop {
            match self.parse_chunk()? {
                Flow::Parsed(ENDG) => break,
                Flow::Parsed(_) => {}
                Flow::EarlyEnd => return Ok(Flow::EarlyEnd),
            }
        }
        self.depth -= 1;
        self.current_mesh = None;
        Ok(Flow::Parsed(BGNG))
    }

    fn parse_reference(&mut self, offset: usize, size: usize) -> Result<Flow> {
        let context = || format!("'rfrn' chunk at {offset:#x}");
        if size != 4 {
            return Err(Error::invalid(format!("bad rfrn size {size}"))).context(context);
        }
        let mut reader = Cursor::new(self.take(size).context(context)?);
        let target: u32 = reader.read_be()?;
        let target_offset = self
            .toc
            .get(&target)
            .and_then(|&o| usize::try_from(o).ok())
            .ok_or_else(|| Error::invalid(format!("reference #{target} is not in the TOC")))
            .context(context)?;

        let jump_back_to = self.pos;
        self.pos = target_offset;
        let flow = self
            .parse_chunk()
            .context(|| format!("reference #{target} from {offset:#x}"));
        self.pos = jump_back_to;
        flow
    }

    fn parse_leaf(&mut self, kind: FourCC, offset: usize, body: &[u8]) -> Result<()> {
        match kind {
            ENDG | ATTR | TXSU => expect_size(body, 0)?,
            KDIF | KXPR => expect_size(body, 12)?,
            SHDR => expect_size(body, 8)?,
            _ => {}
        }
        match kind {
            ENDG | ATTR | TOC => {}
            TMSH => self.parse_tri_mesh(body)?,
            ATAR => {
                let mesh = self.current_mesh_mut()?;
                parse_attribute_array(mesh, body)?;
            }
            KDIF => {
                let [r, g, b] = read_floats::<3>(body)?;
                let mesh = self.current_mesh_mut()?;
                mesh.diffuse_color[..3].copy_from_slice(&[r, g, b]);
            }
            KXPR => {
                let [r, g, b] = read_floats::<3>(body)?;
                if r != g || g != b {
                    return Err(Error::invalid(format!(
                        "transparency colour components differ: {r} {g} {b}"
                    )));
                }
                self.current_mesh_mut()?.diffuse_color[3] = r;
            }
            TXSU => self.parse_texture_shader(offset)?,
            TXMM | TXPM => {
                let shader = self.current_texture()?;
                // Reached again through a reference: keep the first image.
                if shader.pixmap.is_none() {
                    shader.pixmap = Some(parse_pixmap(kind, body)?);
                }
            }
            SHDR => {
                let mut reader = Cursor::new(body);
                let u = UvBoundary::from_raw(reader.read_be()?)?;
                let v = UvBoundary::from_raw(reader.read_be()?)?;
                let shader = self.current_texture()?;
                shader.boundary_u = u;
                shader.boundary_v = v;
            }
            _ => return Err(Error::invalid("unrecognized 3DMF chunk")),
        }
        Ok(())
    }

    fn current_mesh_mut(&mut self) -> Result<&mut TriMesh> {
        self.current_mesh
            .and_then(|i| self.file.meshes.get_mut(i))
            .ok_or_else(|| Error::invalid("no mesh to apply this to"))
    }

    /// Pomme's `GetCurrentTextureShader`: the most recently created texture.
    fn current_texture(&mut self) -> Result<&mut TextureShader> {
        self.textures
            .last_mut()
            .ok_or_else(|| Error::invalid("texture image without a texture shader"))
    }

    /// Port of `Q3MetaFileParser::Parse_tmsh` and the `tmsh` case of
    /// `Parse1Chunk`.
    fn parse_tri_mesh(&mut self, body: &[u8]) -> Result<()> {
        if self.current_mesh.is_some() {
            return Err(Error::invalid("nested meshes are not supported"));
        }
        let mut reader = Reader::new(body);
        let header: TriMeshHeader = reader.read()?;
        if header.edge_count != 0 || header.edge_attribute_count != 0 {
            return Err(Error::invalid("mesh edges are not supported"));
        }

        let point_count = header.point_count as usize;
        let triangle_count = header.triangle_count as usize;
        let index_size = if point_count <= 0xFF {
            1
        } else if point_count <= 0xFFFF {
            2
        } else {
            4
        };
        let index_bytes = reader.array(triangle_count, 3 * index_size)?;
        let triangles: Vec<[u32; 3]> = match index_size {
            1 => index_bytes
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.map(u32::from))
                .collect(),
            2 => index_bytes
                .as_chunks::<6>()
                .0
                .iter()
                .map(|t| {
                    let i = |n: usize| u32::from(u16::from_be_bytes([t[n], t[n + 1]]));
                    [i(0), i(2), i(4)]
                })
                .collect(),
            _ => index_bytes
                .as_chunks::<12>()
                .0
                .iter()
                .map(|t| {
                    let i = |n: usize| u32::from_be_bytes([t[n], t[n + 1], t[n + 2], t[n + 3]]);
                    [i(0), i(4), i(8)]
                })
                .collect(),
        };
        if let Some(bad) = triangles
            .iter()
            .flatten()
            .find(|&&i| i as usize >= point_count)
        {
            return Err(Error::invalid(format!(
                "point index {bad} out of range ({point_count} points)"
            )));
        }

        let positions = reader.floats::<3>(point_count)?;
        let [min, max] = <[[f32; 3]; 2]>::try_from(reader.floats::<3>(2)?)
            .map_err(|_| Error::invalid("bounding box"))?;
        let is_empty: u32 = reader.read()?;
        reader.finish()?;

        let mesh = TriMesh::new(
            positions,
            triangles,
            BoundingBox {
                min,
                max,
                is_empty: is_empty != 0,
            },
        );
        let index = self.file.meshes.len();
        self.file.meshes.push(mesh);
        self.current_mesh = Some(index);

        if self.file.groups.is_empty() {
            self.file.groups.push(Group::default());
        }
        if let Some(group) = self.file.groups.last_mut() {
            group.meshes.push(index);
        }
        Ok(())
    }

    /// The `txsu` case of `Parse1Chunk`.
    fn parse_texture_shader(&mut self, offset: usize) -> Result<()> {
        let texture = match self.known_textures.get(&offset) {
            Some(&texture) => texture,
            None => {
                let texture = self.textures.len();
                self.textures.push(TextureShader::default());
                self.known_textures.insert(offset, texture);
                texture
            }
        };
        if let Some(index) = self.current_mesh {
            let mesh = self.current_mesh_mut()?;
            if mesh.texture.is_some() {
                return Err(Error::invalid(format!(
                    "mesh {index} already has a texture"
                )));
            }
            mesh.texture = Some(texture);
        }
        Ok(())
    }
}

fn expect_size(body: &[u8], size: usize) -> Result<()> {
    if body.len() == size {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "chunk size is {}, expected {size}",
            body.len()
        )))
    }
}

/// Port of `Q3MetaFileParser::Parse_atar`.
fn parse_attribute_array(mesh: &mut TriMesh, body: &[u8]) -> Result<()> {
    let mut reader = Reader::new(body);
    let header: AttributeArrayHeader = reader.read()?;
    let kind = header.attribute_type;
    if header.zero != 0 {
        return Err(Error::invalid("attribute array: expected zero"));
    }
    if !(1..ATTRIBUTE_TYPE_COUNT).contains(&kind) {
        return Err(Error::invalid(format!("illegal attribute type {kind}")));
    }
    if header.position_of_array > 2 {
        return Err(Error::invalid(format!(
            "illegal attribute position {}",
            header.position_of_array
        )));
    }
    if header.use_flag > 1 {
        return Err(Error::invalid(format!(
            "unrecognized attribute use flag {}",
            header.use_flag
        )));
    }

    let point_count = mesh.positions.len();
    let already = || Error::invalid(format!("mesh already has attribute type {kind}"));
    match (header.position_of_array, kind) {
        (PER_POINT, ATTRIBUTE_SURFACE_UV | ATTRIBUTE_SHADING_UV) => {
            if mesh.uvs.is_some() {
                return Err(already());
            }
            let uvs = reader.floats::<2>(point_count)?;
            mesh.uvs = Some(uvs.into_iter().map(|[u, v]| [u, 1.0 - v]).collect());
        }
        (PER_POINT, ATTRIBUTE_NORMAL) => {
            if header.position_in_array != 0 {
                return Err(Error::invalid("normals must have position in array 0"));
            }
            if mesh.normals.is_some() {
                return Err(already());
            }
            mesh.normals = Some(reader.floats::<3>(point_count)?);
        }
        (PER_POINT, ATTRIBUTE_DIFFUSE_COLOR) => {
            if mesh.colors.is_some() {
                return Err(already());
            }
            mesh.colors = Some(reader.floats::<3>(point_count)?);
        }
        (PER_TRIANGLE, ATTRIBUTE_NORMAL) => {
            // Face normals: the game only uses vertex normals.
            reader.array(mesh.triangles.len(), 3 * 4)?;
        }
        (position, _) => {
            return Err(Error::invalid(format!(
                "unsupported attribute type {kind} at position {position}"
            )));
        }
    }
    reader.finish()
}

/// Port of `Q3MetaFileParser::ParsePixmap`.
fn parse_pixmap(kind: FourCC, body: &[u8]) -> Result<Pixmap> {
    let mut reader = Reader::new(body);
    let (pixel_type, bit_order, byte_order, width, height, row_bytes) = if kind == TXMM {
        let h: MipmapHeader = reader.read()?;
        if h.use_mipmapping != 0 {
            return Err(Error::invalid("mipmapped textures are not supported"));
        }
        if h.offset != 0 {
            return Err(Error::invalid("texture offsets are not supported"));
        }
        (
            h.pixel_type,
            h.bit_order,
            h.byte_order,
            h.width,
            h.height,
            h.row_bytes,
        )
    } else {
        let h: PixmapHeader = reader.read()?;
        (
            h.pixel_type,
            h.bit_order,
            h.byte_order,
            h.width,
            h.height,
            h.row_bytes,
        )
    };

    if bit_order != ENDIAN_BIG {
        return Err(Error::invalid(format!("unsupported bit order {bit_order}")));
    }
    let little_endian = match byte_order {
        ENDIAN_BIG => false,
        ENDIAN_LITTLE => true,
        _ => return Err(Error::invalid(format!("unknown byte order {byte_order}"))),
    };
    let pixel_type = PixelType::from_raw(pixel_type)?;

    let bytes_per_pixel = pixel_type.bytes_per_pixel();
    let (width_px, height_px, row_bytes) = (width as usize, height as usize, row_bytes as usize);
    let image_size = row_bytes
        .checked_mul(height_px)
        .map(|size| size.next_multiple_of(4))
        .ok_or_else(|| Error::invalid("image size overflows"))?;
    if reader.remaining() != image_size {
        return Err(Error::invalid(format!(
            "{width}x{height} image with {row_bytes} bytes per row needs {image_size} bytes, chunk has {}",
            reader.remaining()
        )));
    }
    let trimmed_row_bytes = width_px
        .checked_mul(bytes_per_pixel)
        .filter(|&trimmed| trimmed <= row_bytes)
        .ok_or_else(|| {
            Error::invalid(format!(
                "{width} pixels do not fit in {row_bytes} bytes per row"
            ))
        })?;

    let mut pixels = Vec::with_capacity(width_px * height_px);
    for _ in 0..height_px {
        let row = reader.bytes(row_bytes)?;
        let row = &row[..trimmed_row_bytes];
        if bytes_per_pixel == 2 {
            pixels.extend(row.as_chunks::<2>().0.iter().map(|&b| {
                u32::from(if little_endian {
                    u16::from_le_bytes(b)
                } else {
                    u16::from_be_bytes(b)
                })
            }));
        } else {
            pixels.extend(row.as_chunks::<4>().0.iter().map(|&b| {
                if little_endian {
                    u32::from_le_bytes(b)
                } else {
                    u32::from_be_bytes(b)
                }
            }));
        }
    }

    if let Some(alpha_mask) = pixel_type.alpha_mask() {
        apply_edge_padding(&mut pixels, width_px, height_px, alpha_mask);
    }

    Ok(Pixmap {
        width,
        height,
        pixel_type,
        pixels,
    })
}

/// Bleeds colour into fully transparent black pixels (value 0) from their
/// neighbours, keeping them transparent. This stops bilinear filtering from
/// darkening the edges of alpha-tested and blended textures.
///
/// Port of `_EdgePadding` (original/extern/Pomme/src/QD3D/QD3D.cpp).
fn apply_edge_padding(pixels: &mut [u32], width: usize, height: usize, alpha_mask: u32) {
    if width == 0 || height == 0 || pixels.len() != width * height {
        return;
    }
    let color = !alpha_mask;
    for _ in 0..EDGE_PADDING_REPEAT {
        for row in pixels.chunks_exact_mut(width) {
            for x in 0..width - 1 {
                if row[x] == 0 {
                    row[x] = row[x + 1] & color;
                }
            }
            for x in (1..width).rev() {
                if row[x] == 0 {
                    row[x] = row[x - 1] & color;
                }
            }
        }
        for x in 0..width {
            for y in 0..height - 1 {
                let i = y * width + x;
                if pixels[i] == 0 {
                    pixels[i] = pixels[i + width] & color;
                }
            }
            for y in (1..height).rev() {
                let i = y * width + x;
                if pixels[i] == 0 {
                    pixels[i] = pixels[i - width] & color;
                }
            }
        }
    }
}

/// Reads one `f32` triple, quadruple etc. from an exactly sized body.
fn read_floats<const N: usize>(body: &[u8]) -> Result<[f32; N]> {
    let mut reader = Reader::new(body);
    let floats = reader.floats::<N>(1)?;
    reader.finish()?;
    floats
        .first()
        .copied()
        .ok_or_else(|| Error::invalid("missing values"))
}

/// A bounds-checked big-endian reader over one chunk body.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let bytes = self
            .pos
            .checked_add(len)
            .and_then(|end| self.data.get(self.pos..end))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{len} bytes at {} run past the end of the {}-byte chunk",
                    self.pos,
                    self.data.len()
                ))
            })?;
        self.pos += len;
        Ok(bytes)
    }

    /// The bytes of `count` elements of `element_size` bytes. Checked before
    /// anything is allocated, so corrupt counts give an error, not an abort.
    fn array(&mut self, count: usize, element_size: usize) -> Result<&'a [u8]> {
        let len = count
            .checked_mul(element_size)
            .ok_or_else(|| Error::invalid(format!("array of {count} elements overflows")))?;
        self.bytes(len)
    }

    fn floats<const N: usize>(&mut self, count: usize) -> Result<Vec<[f32; N]>> {
        let bytes = self.array(count, N * 4)?;
        let floats: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_be_bytes(b))
            .collect();
        Ok(floats.as_chunks::<N>().0.to_vec())
    }

    fn read<T: for<'b> BinRead<Args<'b> = ()>>(&mut self) -> Result<T> {
        let mut cursor = Cursor::new(self.data.get(self.pos..).unwrap_or_default());
        let value = cursor.read_be()?;
        self.pos += cursor.position() as usize;
        Ok(value)
    }

    /// Fails unless the whole body was read.
    fn finish(&self) -> Result<()> {
        match self.remaining() {
            0 => Ok(()),
            extra => Err(Error::invalid(format!(
                "{extra} unexpected bytes at the end of the chunk"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;

    fn tdmf_files() -> Vec<std::path::PathBuf> {
        let dir = original_data_dir();
        let mut files: Vec<_> = ["Models", "Skeletons"]
            .iter()
            .flat_map(|sub| std::fs::read_dir(dir.join(sub)).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "3dmf"))
            .collect();
        files.sort();
        files
    }

    fn model(name: &str) -> MetaFile {
        open(original_data_dir().join("Models").join(name)).unwrap()
    }

    fn skeleton(name: &str) -> MetaFile {
        open(original_data_dir().join("Skeletons").join(name)).unwrap()
    }

    #[test]
    fn parses_every_original_3dmf() {
        let files = tdmf_files();
        assert_eq!(
            files.len(),
            16 + 24,
            "expected 16 model and 24 skeleton files"
        );
        for path in files {
            let file = open(&path).unwrap_or_else(|e| panic!("{e:?}"));
            let name = path.display();
            assert!(!file.meshes.is_empty(), "{name} has no meshes");
            for (i, mesh) in file.meshes.iter().enumerate() {
                let n = mesh.positions.len();
                assert!(!mesh.triangles.is_empty(), "{name} mesh {i}");
                assert!(mesh.triangles.iter().flatten().all(|&v| (v as usize) < n));
                assert!(mesh.normals.as_ref().is_none_or(|a| a.len() == n));
                assert!(mesh.uvs.as_ref().is_none_or(|a| a.len() == n));
                assert!(mesh.colors.as_ref().is_none_or(|a| a.len() == n));
                assert!(mesh.texture.is_none_or(|t| t < file.textures.len()));
            }
            // Every mesh belongs to exactly one group.
            let mut grouped: Vec<_> = file.groups.iter().flat_map(|g| &g.meshes).collect();
            grouped.sort();
            assert!(
                grouped.into_iter().copied().eq(0..file.meshes.len()),
                "{name}"
            );
            for texture in &file.textures {
                let p = &texture.pixmap;
                assert_eq!(p.pixels.len(), (p.width * p.height) as usize, "{name}");
                assert_eq!(p.to_rgba8().len(), p.pixels.len() * 4);
            }
        }
    }

    #[test]
    fn grouped_models_have_non_empty_groups_within_game_limit() {
        // LoadGrouped3DMF asserts 0 < groups <= MAX_OBJECTS_IN_GROUP and that
        // no group is empty.
        const MAX_OBJECTS_IN_GROUP: usize = 64;
        for path in tdmf_files()
            .iter()
            .filter(|p| p.parent().unwrap().ends_with("Models"))
        {
            let file = open(path).unwrap();
            assert!((1..=MAX_OBJECTS_IN_GROUP).contains(&file.groups.len()));
            assert!(
                file.groups.iter().all(|g| !g.meshes.is_empty()),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn global_models_object_counts_match_enums() {
        // GLOBAL1_MObjType_LadyBugPost = 10 and GLOBAL2_MObjType_Tick = 8
        // (original/src/Headers/mobjtypes.h) are the last of each enum.
        assert_eq!(model("Global_Models1.3dmf").groups.len(), 11);
        assert_eq!(model("Global_Models2.3dmf").groups.len(), 9);
    }

    #[test]
    fn beehive_tubes_have_two_meshes() {
        // UpdateHoneyTubeTextureAnimation (original/src/Items/Items2.c)
        // asserts HIVE_MObjType_BentTube..=TaperTube (20..=23) have two meshes
        // each, and scrolls the UVs of the second.
        let file = model("BeeHive_Models.3dmf");
        // HIVE_MObjType_Stinger = 26 is the last object type.
        assert_eq!(file.groups.len(), 27);
        for tube in &file.groups[20..=23] {
            assert_eq!(tube.meshes.len(), 2);
            let inner = &file.meshes[tube.meshes[1]];
            assert!(inner.uvs.is_some() && inner.texture.is_some());
        }
    }

    #[test]
    fn ant_king_has_the_meshes_the_game_indexes() {
        // LoadASkeleton (original/src/Skeleton/SkeletonObj.c) indexes the
        // Ant King's decomposed meshes 3, 8 and 9, and decomposition walks
        // MetaFile::meshes in order.
        let file = skeleton("AntKing.3dmf");
        assert!(file.meshes.len() >= 10);
        // DecomposeATriMesh dereferences the vertex normals of every mesh.
        for path in tdmf_files()
            .iter()
            .filter(|p| p.parent().unwrap().ends_with("Skeletons"))
        {
            let file = open(path).unwrap();
            assert!(
                file.meshes.iter().all(|m| m.normals.is_some()),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn foot_skeleton_layout() {
        // Checked against a hex dump: an outer cntr holding two meshes of 40
        // and 242 points, each followed by vertex normals and an attribute
        // set cntr with a kdif colour.
        let file = skeleton("Foot.3dmf");
        assert_eq!(file.meshes.len(), 2);
        // Pomme's grouping rule gives odd groups here: the first tmsh creates
        // a group, and each attribute set cntr (at depth 1) opens another.
        // Skeleton code only uses the flat mesh list.
        let groups: Vec<_> = file.groups.iter().map(|g| g.meshes.clone()).collect();
        assert_eq!(groups, [vec![0], vec![1], vec![]]);
        assert_eq!(file.meshes[0].positions.len(), 40);
        assert_eq!(file.meshes[0].triangles.len(), 64);
        assert_eq!(file.meshes[1].positions.len(), 242);
        assert_eq!(file.meshes[1].triangles.len(), 474);
        assert!(file.textures.is_empty());
        assert!(file.meshes.iter().all(|m| m.diffuse_color != [1.0; 4]));
    }

    #[test]
    fn global_models2_textures_and_vertex_colors() {
        let file = model("Global_Models2.3dmf");
        // The first object's first mesh: 58 points, 112 triangles, a 64x64
        // ARGB16 texture with 128 bytes per row.
        let mesh = &file.meshes[file.groups[0].meshes[0]];
        assert_eq!((mesh.positions.len(), mesh.triangles.len()), (58, 112));
        let texture = &file.textures[mesh.texture.unwrap()];
        assert_eq!((texture.pixmap.width, texture.pixmap.height), (64, 64));
        assert_eq!(texture.pixmap.pixel_type, PixelType::Argb16);
        assert_eq!(texture.texturing_mode(), TexturingMode::AlphaTest);
        // Pomme notes per-vertex diffuse colours are used in this file.
        assert!(file.meshes.iter().any(|m| m.colors.is_some()));
    }

    #[test]
    fn references_share_textures() {
        // AntHill_Models refers back to earlier texture shaders through the
        // TOC, so there are more textured meshes than textures.
        let file = model("AntHill_Models.3dmf");
        let textured = file.meshes.iter().filter(|m| m.texture.is_some()).count();
        assert_eq!(file.textures.len(), 6);
        assert!(textured > file.textures.len());
        // The texture at 0x5f34 is followed by `shdr 1 1`.
        assert!(
            file.textures
                .iter()
                .any(|t| t.boundary_u == UvBoundary::Clamp && t.boundary_v == UvBoundary::Clamp)
        );
    }

    #[test]
    fn group_bounds() {
        let file = model("Global_Models1.3dmf");
        let group = &file.groups[0];
        let bbox = BoundingBox::of_meshes(file.group_meshes(group));
        let sphere = BoundingSphere::of_meshes(file.group_meshes(group));
        assert!(!bbox.is_empty && !sphere.is_empty);
        for p in file.group_meshes(group).flat_map(|m| &m.positions) {
            for ((min, max), v) in bbox.min.iter().zip(bbox.max).zip(p) {
                assert!(min <= v && v <= &max);
            }
        }
        assert!(sphere.radius > 0.0);
        assert!(BoundingBox::of_meshes([]).is_empty);
    }

    #[test]
    fn edge_padding_bleeds_colour_but_not_alpha() {
        let mut pixels = vec![0, 0, 0x8000 | 0x7C00, 0];
        apply_edge_padding(&mut pixels, 4, 1, 0x8000);
        assert_eq!(pixels, vec![0x7C00, 0x7C00, 0x8000 | 0x7C00, 0x7C00]);
    }

    #[test]
    fn rgba8_conversion() {
        let pixmap = |pixel_type, pixels| Pixmap {
            width: 1,
            height: 1,
            pixel_type,
            pixels,
        };
        assert_eq!(
            pixmap(PixelType::Argb16, vec![0x8000 | 0x7C00]).to_rgba8(),
            [255, 0, 0, 255]
        );
        assert_eq!(
            pixmap(PixelType::Argb16, vec![0x001F]).to_rgba8(),
            [0, 0, 255, 0]
        );
        assert_eq!(
            pixmap(PixelType::Rgb16, vec![0x03E0]).to_rgba8(),
            [0, 255, 0, 255]
        );
        assert_eq!(
            pixmap(PixelType::Argb32, vec![0x80112233]).to_rgba8(),
            [0x11, 0x22, 0x33, 0x80]
        );
        assert_eq!(
            pixmap(PixelType::Rgb32, vec![0x00112233]).to_rgba8(),
            [0x11, 0x22, 0x33, 0xFF]
        );
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse(b"not a 3DMF file").is_err());
        let header = |toc: u64| {
            let mut h = b"3DMF\0\0\0\x10\0\x01\0\x06\0\0\0\0".to_vec();
            h.extend(toc.to_be_bytes());
            h
        };
        // Empty file: no chunks, no meshes.
        assert_eq!(parse(&header(0)).unwrap(), MetaFile::default());
        // Truncated chunk.
        let mut data = header(0);
        data.extend(b"tmsh\0\0\x01\0");
        assert!(parse(&data).is_err());
        // Unknown chunk type.
        let mut data = header(0);
        data.extend(b"what\0\0\0\0");
        assert!(parse(&data).is_err());
        // A reference to itself must not overflow the stack.
        let mut data = header(24 + 12);
        data.extend(b"rfrn\0\0\0\x04\0\0\0\x01");
        data.extend(b"toc \0\0\0\x2c");
        data.extend([0; 16]);
        data.extend([0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 1]);
        data.extend([0, 0, 0, 1]);
        data.extend(24_u64.to_be_bytes());
        data.extend(b"rfrn");
        let error = parse(&data).unwrap_err();
        assert!(
            format!("{error:?}").contains("nests too deeply"),
            "{error:?}"
        );
        // A mesh with a huge point count does not allocate.
        let mut data = header(0);
        data.extend(b"tmsh\0\0\0\x34");
        data.extend([0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        data.extend([0xFF; 4]);
        data.extend([0; 32]);
        assert!(parse(&data).is_err());
    }

    #[test]
    fn zero_chunk_type_ends_parsing() {
        let mut data = b"3DMF\0\0\0\x10\0\x01\0\x06\0\0\0\0".to_vec();
        data.extend(0_u64.to_be_bytes());
        data.extend([0; 8]);
        data.extend(b"garbage");
        assert_eq!(parse(&data).unwrap(), MetaFile::default());
    }
}
