//! Classic Mac OS resource forks, as stored in the AppleDouble `.rsrc` files of
//! the original data (skeletons and terrain).
//!
//! Port of `FSpOpenResFile` (original/extern/Pomme/src/Files/Resources.cpp)
//! and `ADFJumpToResourceFork` (original/extern/Pomme/src/Files/HostVolume.cpp).
//! The layout is documented in *Inside Macintosh: More Macintosh Toolbox*,
//! chapter 1, "Resource Manager Reference".

use std::collections::BTreeMap;
use std::io::{Cursor, Seek, SeekFrom};
use std::path::Path;

use binrw::{BinRead, BinReaderExt, binread};

use crate::error::{Error, Result, ResultExt, read_file};
use crate::four_cc::FourCC;
use crate::mac_roman;

/// One resource: its data and optional name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub kind: FourCC,
    pub id: i16,
    pub name: Option<String>,
    pub data: Vec<u8>,
}

/// All resources of one resource fork, loaded into memory.
#[derive(Debug, Clone, Default)]
pub struct ResourceFork {
    resources: BTreeMap<(FourCC, i16), Resource>,
}

impl ResourceFork {
    /// Reads an AppleDouble file (such as `Ant.skeleton.rsrc`) and parses its
    /// resource fork.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = read_file(path)?;
        Self::from_apple_double(&bytes).context(|| format!("in {}", path.display()))
    }

    /// Parses the resource fork inside an AppleDouble file.
    pub fn from_apple_double(bytes: &[u8]) -> Result<Self> {
        let fork = apple_double_entry(bytes, APPLE_DOUBLE_RESOURCE_FORK)?;
        Self::parse(fork)
    }

    /// Parses a bare resource fork.
    pub fn parse(fork: &[u8]) -> Result<Self> {
        let mut reader = Cursor::new(fork);
        let header: ForkHeader = reader.read_be()?;

        let map_start = u64::from(header.map_offset);
        reader.seek(SeekFrom::Start(map_start))?;
        let map: MapHeader = reader.read_be()?;
        let type_list_start = map_start + u64::from(map.type_list_offset);
        let name_list_start = map_start + u64::from(map.name_list_offset);

        reader.seek(SeekFrom::Start(type_list_start))?;
        // Counts are stored minus one, so an empty fork stores 0xFFFF.
        let type_count = reader.read_be::<u16>()?.wrapping_add(1);
        let types: Vec<TypeEntry> = (0..type_count)
            .map(|_| reader.read_be())
            .collect::<Result<_, _>>()?;

        let mut resources = BTreeMap::new();
        for entry in types {
            reader.seek(SeekFrom::Start(
                type_list_start + u64::from(entry.ref_list_offset),
            ))?;
            let refs: Vec<RefEntry> = (0..u32::from(entry.count_minus_one) + 1)
                .map(|_| reader.read_be())
                .collect::<Result<_, _>>()?;

            for r in refs {
                let resource = read_resource(&mut reader, &header, name_list_start, entry.kind, r)
                    .context(|| format!("resource {:?} {}", entry.kind, r.id))?;
                resources.insert((entry.kind, r.id), resource);
            }
        }

        Ok(Self { resources })
    }

    pub fn get(&self, kind: FourCC, id: i16) -> Option<&Resource> {
        self.resources.get(&(kind, id))
    }

    /// Like [`get`](Self::get), but reports a missing resource as an error.
    pub fn require(&self, kind: FourCC, id: i16) -> Result<&Resource> {
        self.get(kind, id)
            .ok_or_else(|| Error::invalid(format!("missing resource {kind:?} {id}")))
    }

    /// Resources of one type, in ascending ID order.
    pub fn of_kind(&self, kind: FourCC) -> impl Iterator<Item = &Resource> {
        self.resources
            .range((kind, i16::MIN)..=(kind, i16::MAX))
            .map(|(_, r)| r)
    }

    /// All resources, ordered by type and then ID.
    pub fn iter(&self) -> impl Iterator<Item = &Resource> {
        self.resources.values()
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

fn read_resource(
    reader: &mut Cursor<&[u8]>,
    header: &ForkHeader,
    name_list_start: u64,
    kind: FourCC,
    r: RefEntry,
) -> Result<Resource> {
    if r.attributes & ATTRIBUTE_COMPRESSED != 0 {
        return Err(Error::invalid("compressed resources are not supported"));
    }

    let name = if r.name_offset == NO_NAME {
        None
    } else {
        reader.seek(SeekFrom::Start(name_list_start + u64::from(r.name_offset)))?;
        let len: u8 = reader.read_be()?;
        let bytes = read_bytes(reader, usize::from(len))?;
        Some(mac_roman::decode(bytes))
    };

    let data_start = u64::from(header.data_offset) + u64::from(r.data_offset());
    reader.seek(SeekFrom::Start(data_start))?;
    let len: u32 = reader.read_be()?;
    let data = read_bytes(reader, len as usize)?.to_vec();

    Ok(Resource {
        kind,
        id: r.id,
        name,
        data,
    })
}

/// Borrows `len` bytes at the cursor's position and advances past them.
fn read_bytes<'a>(reader: &mut Cursor<&'a [u8]>, len: usize) -> Result<&'a [u8]> {
    let all: &'a [u8] = reader.get_ref();
    let start = usize::try_from(reader.position()).unwrap_or(usize::MAX);
    let bytes = start
        .checked_add(len)
        .and_then(|end| all.get(start..end))
        .ok_or_else(|| Error::invalid(format!("{len} bytes at {start} run past the end")))?;
    reader.set_position((start + len) as u64);
    Ok(bytes)
}

/// AppleDouble entry ID of the resource fork.
const APPLE_DOUBLE_RESOURCE_FORK: u32 = 2;
/// Name offset meaning "this resource has no name".
const NO_NAME: u16 = 0xFFFF;
/// `resCompressed`: the data is compressed with a decompressor resource.
const ATTRIBUTE_COMPRESSED: u8 = 0x01;

/// Returns the bytes of one entry in an AppleDouble file.
fn apple_double_entry(bytes: &[u8], id: u32) -> Result<&[u8]> {
    let header: AppleDoubleHeader = Cursor::new(bytes).read_be()?;
    let entry = header
        .entries
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| Error::invalid(format!("AppleDouble file has no entry {id}")))?;
    let start = entry.offset as usize;
    start
        .checked_add(entry.length as usize)
        .and_then(|end| bytes.get(start..end))
        .ok_or_else(|| Error::invalid(format!("AppleDouble entry {id} runs past the end")))
}

#[binread]
#[br(big, magic = 0x0005_1607_0002_0000_u64)]
struct AppleDoubleHeader {
    _filler: [u8; 16],
    #[br(temp)]
    count: u16,
    #[br(count = count)]
    entries: Vec<AppleDoubleEntry>,
}

#[derive(BinRead)]
#[br(big)]
struct AppleDoubleEntry {
    id: u32,
    offset: u32,
    length: u32,
}

#[derive(BinRead)]
#[br(big)]
struct ForkHeader {
    data_offset: u32,
    map_offset: u32,
    _data_length: u32,
    _map_length: u32,
}

#[derive(BinRead)]
#[br(big)]
struct MapHeader {
    // Copy of the fork header, next-map handle and file reference number;
    // only meaningful in memory on a real Mac.
    _reserved: [u8; 16 + 4 + 2],
    _attributes: u16,
    type_list_offset: u16,
    name_list_offset: u16,
}

#[derive(BinRead)]
#[br(big)]
struct TypeEntry {
    kind: FourCC,
    count_minus_one: u16,
    ref_list_offset: u16,
}

#[derive(BinRead, Clone, Copy)]
#[br(big)]
struct RefEntry {
    id: i16,
    name_offset: u16,
    /// High byte: attributes. Low three bytes: data offset.
    attributes_and_offset: u32,
    _handle: u32,
    #[br(calc = (attributes_and_offset >> 24) as u8)]
    attributes: u8,
}

impl RefEntry {
    fn data_offset(&self) -> u32 {
        self.attributes_and_offset & 0x00FF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;

    fn rsrc_files() -> Vec<std::path::PathBuf> {
        let dir = original_data_dir();
        let mut files: Vec<_> = ["Skeletons", "Terrain"]
            .iter()
            .flat_map(|sub| std::fs::read_dir(dir.join(sub)).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "rsrc"))
            .collect();
        files.sort();
        files
    }

    #[test]
    fn parses_every_original_resource_fork() {
        let files = rsrc_files();
        assert!(files.len() >= 30, "expected skeleton and terrain forks");
        for path in files {
            let fork = ResourceFork::open(&path).unwrap_or_else(|e| panic!("{e:?}"));
            assert!(!fork.is_empty(), "{} has no resources", path.display());
        }
    }

    #[test]
    fn skeleton_has_header_resource() {
        let fork =
            ResourceFork::open(original_data_dir().join("Skeletons/Ant.skeleton.rsrc")).unwrap();
        // LoadBonesReferenceModel/ReadDataFromSkeletonFile read 'Hedr' 1000 first.
        let header = fork.require(FourCC::new(b"Hedr"), 1000).unwrap();
        assert!(!header.data.is_empty());
        assert!(fork.of_kind(FourCC::new(b"Bone")).count() > 1);
    }

    #[test]
    fn of_kind_is_sorted_and_filtered() {
        let fork = ResourceFork::open(original_data_dir().join("Terrain/Lawn.ter.rsrc")).unwrap();
        for kind in fork.iter().map(|r| r.kind) {
            let ids: Vec<_> = fork.of_kind(kind).map(|r| r.id).collect();
            assert!(ids.is_sorted());
            assert!(fork.of_kind(kind).all(|r| r.kind == kind));
        }
    }

    #[test]
    fn rejects_non_apple_double() {
        assert!(ResourceFork::from_apple_double(b"not an AppleDouble file").is_err());
    }
}
