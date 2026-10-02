//! Skeleton definitions: bone hierarchy, skinning tables and keyframe
//! animations, from the `.skeleton.rsrc` files in `Data/Skeletons`.
//!
//! Port of `ReadDataFromSkeletonFile` (original/src/System/File.c), with the
//! resource structs from original/src/Headers/file.h and structs.h and their
//! byte layouts from `STRUCTFORMAT_*` in original/src/Headers/structformats.h.
//! How the data is used is in original/src/Skeleton/{Bones,SkeletonAnim,
//! SkeletonJoints}.c.
//!
//! # Format
//!
//! Each skeleton is a resource fork (see [`crate::rsrc`]). All values are
//! big-endian; there is no padding unless noted.
//!
//! | Resource | ID | Contents |
//! |---|---|---|
//! | `Hedr` | 1000 | `i16 version` (must be `0x0110`), `i16 numAnims`, `i16 numJoints`, `i16 num3DMFLimbs` (unused) |
//! | `alis` | 1000 | Mac alias to the skeleton's `.3dmf`; ignored, the geometry is the `.3dmf` with the same base name |
//! | `Bone` | 1000 + bone | `i32 parentBone` (-1 = root), `char name[32]` (a Pascal string: length byte, then text, then junk), `f32 coord[3]` (absolute, not relative to the parent), `u16 numPoints`, `u16 numNormals`, `u32 reserved[8]` (84 bytes) |
//! | `BonP` | 1000 + bone | `u16[numPoints]`: indices of the decomposed points this bone moves |
//! | `BonN` | 1000 + bone | `u16[numNormals]`: indices of the decomposed normals this bone rotates |
//! | `RelP` | 1000 | `f32[3]` per decomposed point: its offset from the bone that owns it |
//! | `AnHd` | 1000 + anim | `u8 nameLength`, `char name[32]`, 1 pad byte, `i16 numAnimEvents` (36 bytes) |
//! | `Evnt` | 1000 + anim | `numAnimEvents` × (`i16 time`, `u8 type`, `u8 value`) |
//! | `NumK` | 1000 + anim | `i8[numJoints]`: keyframe count of each joint in this animation |
//! | `KeyF` | 1000 + anim × 100 + joint | `numKeyframes` × (`i32 tick`, `i32 accelerationMode`, `f32 coord[3]`, `f32 rotation[3]`, `f32 scale[3]`) (44 bytes each) |
//!
//! "Decomposed" points and normals are not the raw vertices of the `.3dmf`:
//! `DecomposeATriMesh` (original/src/Skeleton/Bones.c) walks every vertex of
//! every trimesh in file order and merges points (and, separately, unit
//! normals) that are close enough to an earlier one. The indices in `BonP` and
//! `BonN`, and the order of `RelP`, refer to those merged lists, so `RelP` has
//! exactly one entry per decomposed point.
//!
//! Units are the original's: coordinates in world units, rotations in radians
//! (applied by `Q3Matrix4x4_SetRotate_XYZ`), and time in animation ticks, of
//! which the game plays 30 per second at normal speed.

use std::io::Cursor;
use std::path::Path;

use binrw::{BinRead, BinReaderExt};

use crate::error::{Error, Result, ResultExt};
use crate::four_cc::FourCC;
use crate::mac_roman;
use crate::rsrc::ResourceFork;

/// `SKELETON_FILE_VERS_NUM` in File.c: version 1.1.
pub const SKELETON_FILE_VERSION: i16 = 0x0110;

/// Animation ticks per second at an animation speed of 1
/// (`UpdateSkeletonAnimation` advances `30 * gFramesPerSecondFrac` per frame).
pub const TICKS_PER_SECOND: f32 = 30.0;

/// Base resource ID of every per-skeleton, per-bone and per-animation resource.
const BASE_ID: i16 = 1000;

/// A whole skeleton file.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonDefinition {
    /// Bone 0 is the root; every other bone's parent is a valid index.
    pub bones: Vec<Bone>,
    /// For each decomposed point of the skeleton's geometry, its position
    /// relative to the bone that owns it (`DecomposedPointType::boneRelPoint`).
    pub relative_points: Vec<[f32; 3]>,
    pub animations: Vec<Animation>,
}

/// One bone (also called a joint in the original).
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone, or `None` for the root.
    pub parent: Option<usize>,
    /// Absolute position in the bind pose (not relative to the parent).
    pub coord: [f32; 3],
    /// Decomposed points skinned to this bone. A few skeletons (DoodleBug,
    /// Mosquito, WaterBug) list some points on several bones; the original
    /// transforms them once per bone with the same [`relative_points`] offset,
    /// so the bone visited last in `UpdateSkinnedGeometry_Recurse`'s
    /// depth-first walk from bone 0 (see [`SkeletonDefinition::children`])
    /// decides where the point ends up.
    ///
    /// [`relative_points`]: SkeletonDefinition::relative_points
    pub point_indices: Vec<u16>,
    /// Decomposed normals rotated by this bone.
    pub normal_indices: Vec<u16>,
}

/// One animation.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Events in the order the game processes them, which is by time.
    pub events: Vec<AnimEvent>,
    /// Keyframes of each bone, indexed like [`SkeletonDefinition::bones`].
    /// A bone can have no keyframes in an animation.
    pub keyframes: Vec<Vec<Keyframe>>,
}

/// An animation event (`AnimEventType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimEvent {
    /// Tick at which the event fires.
    pub time: i16,
    pub kind: AnimEventKind,
}

/// The `ANIMEVENT_TYPE_*` constants in original/src/Headers/skeletonanim.h,
/// as handled by `UpdateSkeletonAnimation` (original/src/Skeleton/SkeletonAnim.c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimEventKind {
    /// The animation has finished (`AnimHasStopped`).
    Stop,
    /// Jump back to the last marker (or tick 0) and keep playing.
    Loop,
    /// Play backwards to the last marker (or tick 0), then forwards again.
    ZigZag,
    /// Defined by the editor but ignored by the game. `value` is kept because
    /// it presumably names the marker.
    GotoMarker { value: u8 },
    /// Sets the time [`Loop`](Self::Loop) and [`ZigZag`](Self::ZigZag) return to.
    SetMarker,
    /// Plays sound effect `sound`: 0 is the kick, 1 the water bug.
    PlaySound { sound: u8 },
    /// Sets one of the object's general-purpose flags (`ObjNode::Flag[flag]`).
    SetFlag { flag: u8 },
    /// Clears one of the object's general-purpose flags.
    ClearFlag { flag: u8 },
}

impl AnimEventKind {
    fn from_raw(kind: u8, value: u8) -> Result<Self> {
        Ok(match kind {
            0 => Self::Stop,
            1 => Self::Loop,
            2 => Self::ZigZag,
            3 => Self::GotoMarker { value },
            4 => Self::SetMarker,
            5 => Self::PlaySound { sound: value },
            6 => Self::SetFlag { flag: value },
            7 => Self::ClearFlag { flag: value },
            _ => {
                return Err(Error::invalid(format!(
                    "unknown animation event type {kind}"
                )));
            }
        })
    }
}

/// One keyframe of one bone (`JointKeyframeType`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    pub tick: i32,
    /// How to interpolate from this keyframe to the next one.
    pub acceleration: AccelerationMode,
    /// Position relative to the parent bone.
    pub coord: [f32; 3],
    /// Euler angles in radians, applied X then Y then Z.
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

/// The `ACCEL_MODE_*` constants in original/src/Headers/skeletonanim.h, as
/// used by `InterpolateKeyFrames` (original/src/Skeleton/SkeletonAnim.c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelerationMode {
    Linear,
    /// Smoothstep from start to end.
    EaseInOut,
    /// The first half of the smoothstep, stretched: starts slowly.
    EaseIn,
    /// The second half of the smoothstep, stretched: ends slowly.
    EaseOut,
}

impl AccelerationMode {
    fn from_raw(mode: i32) -> Result<Self> {
        Ok(match mode {
            0 => Self::Linear,
            1 => Self::EaseInOut,
            2 => Self::EaseIn,
            3 => Self::EaseOut,
            _ => return Err(Error::invalid(format!("unknown acceleration mode {mode}"))),
        })
    }
}

impl SkeletonDefinition {
    /// Indices of `bone`'s children, in the order `PrimeBoneData`
    /// (original/src/Skeleton/Bones.c) links them.
    pub fn children(&self, bone: usize) -> impl Iterator<Item = usize> + '_ {
        self.bones
            .iter()
            .enumerate()
            .filter(move |(_, b)| b.parent == Some(bone))
            .map(|(i, _)| i)
    }
}

impl Animation {
    /// The last keyframe tick of any bone, i.e. the animation's length.
    /// Port of `CalcMaxKeyFrameTime` (original/src/Skeleton/SkeletonAnim.c).
    pub fn max_tick(&self) -> i32 {
        self.keyframes
            .iter()
            .flatten()
            .map(|k| k.tick)
            .fold(0, i32::max)
    }
}

/// Reads a skeleton from an AppleDouble file such as `Ant.skeleton.rsrc`.
pub fn open(path: impl AsRef<Path>) -> Result<SkeletonDefinition> {
    let path = path.as_ref();
    let fork = ResourceFork::open(path)?;
    parse(&fork).context(|| format!("in {}", path.display()))
}

/// Parses the skeleton resources of a resource fork.
pub fn parse(fork: &ResourceFork) -> Result<SkeletonDefinition> {
    let header: FileHeader = read_one(fork, b"Hedr", BASE_ID)?;
    if header.version != SKELETON_FILE_VERSION {
        return Err(Error::invalid(format!(
            "skeleton version {:#06x} is not {SKELETON_FILE_VERSION:#06x}",
            header.version
        )));
    }
    let num_bones = count(header.num_joints, "bone")?;
    let num_anims = count(header.num_anims, "animation")?;

    let bones = (0..num_bones)
        .map(|i| read_bone(fork, i, num_bones))
        .collect::<Result<Vec<_>>>()?;
    // PrimeBoneData asserts there is a bone, and UpdateSkinnedGeometry that
    // bone 0 is the root its recursion starts from.
    match bones.first() {
        None => return Err(Error::invalid("skeleton has no bones")),
        Some(root) if root.parent.is_some() => {
            return Err(Error::invalid("bone 0 is not the root"));
        }
        Some(_) => {}
    }

    let relative_points = {
        let data = resource_data(fork, b"RelP", BASE_ID)?;
        if data.len() % 12 != 0 {
            return Err(Error::invalid(format!(
                "'RelP' {BASE_ID} has {} bytes, not a whole number of points",
                data.len()
            )));
        }
        read_array::<[f32; 3]>(data, data.len() / 12).context(|| format!("'RelP' {BASE_ID}"))?
    };

    let animations = (0..num_anims)
        .map(|i| read_animation(fork, i, num_bones))
        .collect::<Result<Vec<_>>>()?;

    Ok(SkeletonDefinition {
        bones,
        relative_points,
        animations,
    })
}

fn read_bone(fork: &ResourceFork, index: usize, num_bones: usize) -> Result<Bone> {
    let id = resource_id(index)?;
    let file: FileBone = read_one(fork, b"Bone", id)?;
    let parent = match file.parent_bone {
        NO_PREVIOUS_JOINT => None,
        p => Some(
            usize::try_from(p)
                .ok()
                .filter(|&p| p < num_bones)
                .ok_or_else(|| Error::invalid(format!("bone {index} has invalid parent {p}")))?,
        ),
    };
    Ok(Bone {
        name: pascal_string(&file.name),
        parent,
        coord: file.coord,
        point_indices: read_exact_u16s(fork, b"BonP", id, file.num_points)?,
        normal_indices: read_exact_u16s(fork, b"BonN", id, file.num_normals)?,
    })
}

fn read_animation(fork: &ResourceFork, index: usize, num_bones: usize) -> Result<Animation> {
    let id = resource_id(index)?;
    let header: FileAnimHeader = read_one(fork, b"AnHd", id)?;
    let num_events =
        count(header.num_events, "animation event").context(|| format!("'AnHd' {id}"))?;
    let name_len = usize::from(header.name_length).min(header.name.len());
    let name = mac_roman::decode(&header.name[..name_len]);

    let events = read_array::<FileAnimEvent>(resource_data(fork, b"Evnt", id)?, num_events)
        .and_then(|events| {
            events
                .into_iter()
                .map(|e| {
                    Ok(AnimEvent {
                        time: e.time,
                        kind: AnimEventKind::from_raw(e.kind, e.value)?,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .context(|| format!("'Evnt' {id}"))?;

    let counts = resource_data(fork, b"NumK", id)?;
    let counts = counts.get(..num_bones).ok_or_else(|| {
        Error::invalid(format!(
            "'NumK' {id} has {} entries for {num_bones} bones",
            counts.len()
        ))
    })?;

    let keyframes = counts
        .iter()
        .enumerate()
        .map(|(bone, &n)| {
            // The counts are `signed char` in the original.
            let n = count(i16::from(n as i8), "keyframe").context(|| format!("'NumK' {id}"))?;
            let key_id = resource_id(index * 100 + bone)?;
            read_array::<FileKeyframe>(resource_data(fork, b"KeyF", key_id)?, n)
                .and_then(|kfs| kfs.into_iter().map(Keyframe::try_from).collect())
                .context(|| format!("'KeyF' {key_id}"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Animation {
        name,
        events,
        keyframes,
    })
}

impl TryFrom<FileKeyframe> for Keyframe {
    type Error = Error;

    fn try_from(k: FileKeyframe) -> Result<Self> {
        Ok(Self {
            tick: k.tick,
            acceleration: AccelerationMode::from_raw(k.acceleration_mode)?,
            coord: k.coord,
            rotation: k.rotation,
            scale: k.scale,
        })
    }
}

/// `NO_PREVIOUS_JOINT` in original/src/Headers/skeletonjoints.h.
const NO_PREVIOUS_JOINT: i32 = -1;

/// Resource ID `1000 + offset`, failing if it does not fit in an `i16`.
fn resource_id(offset: usize) -> Result<i16> {
    i16::try_from(offset)
        .ok()
        .and_then(|o| BASE_ID.checked_add(o))
        .ok_or_else(|| Error::invalid(format!("resource ID 1000 + {offset} is out of range")))
}

fn count(n: i16, what: &str) -> Result<usize> {
    usize::try_from(n).map_err(|_| Error::invalid(format!("negative {what} count {n}")))
}

fn resource_data<'a>(fork: &'a ResourceFork, kind: &[u8; 4], id: i16) -> Result<&'a [u8]> {
    Ok(&fork.require(FourCC::new(kind), id)?.data)
}

/// Reads one struct from the start of a resource. Like `UNPACK_STRUCTS`,
/// trailing bytes are ignored.
fn read_one<T>(fork: &ResourceFork, kind: &[u8; 4], id: i16) -> Result<T>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    let data = resource_data(fork, kind, id)?;
    Cursor::new(data)
        .read_be::<T>()
        .context(|| format!("{:?} {id}", FourCC::new(kind)))
}

/// Reads `n` consecutive structs from the start of `data`; trailing bytes are
/// ignored like `UNPACK_STRUCTS` does.
fn read_array<T>(data: &[u8], n: usize) -> Result<Vec<T>>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    let mut reader = Cursor::new(data);
    (0..n).map(|_| Ok(reader.read_be::<T>()?)).collect()
}

/// Reads a `u16` array whose resource must be exactly `n` entries long, as
/// `UNPACK_BE_SCALARS_HANDLE` checks.
fn read_exact_u16s(fork: &ResourceFork, kind: &[u8; 4], id: i16, n: u16) -> Result<Vec<u16>> {
    let data = resource_data(fork, kind, id)?;
    let n = usize::from(n);
    if data.len() != n * 2 {
        return Err(Error::invalid(format!(
            "{:?} {id} has {} bytes, expected {n} indices",
            FourCC::new(kind),
            data.len()
        )));
    }
    read_array(data, n)
}

/// Decodes a length-prefixed string stored in a fixed-size field. The editor
/// left stale bytes after the string, so the length must be honoured.
fn pascal_string(field: &[u8]) -> String {
    match field.split_first() {
        Some((&len, rest)) => mac_roman::decode(&rest[..usize::from(len).min(rest.len())]),
        None => String::new(),
    }
}

/// `SkeletonFile_Header_Type`, format `>4h`.
#[derive(BinRead)]
#[br(big)]
struct FileHeader {
    version: i16,
    num_anims: i16,
    num_joints: i16,
    _num_3dmf_limbs: i16,
}

/// `File_BoneDefinitionType`, format `>i32c3f2H8I`.
#[derive(BinRead)]
#[br(big)]
struct FileBone {
    parent_bone: i32,
    name: [u8; 32],
    coord: [f32; 3],
    num_points: u16,
    num_normals: u16,
    _reserved: [u32; 8],
}

/// `SkeletonFile_AnimHeader_Type`, format `>B32cxh`.
#[derive(BinRead)]
#[br(big)]
struct FileAnimHeader {
    name_length: u8,
    name: [u8; 32],
    #[br(pad_before = 1)]
    num_events: i16,
}

/// `AnimEventType`, format `>hBB`.
#[derive(BinRead)]
#[br(big)]
struct FileAnimEvent {
    time: i16,
    kind: u8,
    value: u8,
}

/// `JointKeyframeType`, format `>ii9f`.
#[derive(BinRead)]
#[br(big)]
struct FileKeyframe {
    tick: i32,
    acceleration_mode: i32,
    coord: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;

    fn skeleton_files() -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(original_data_dir().join("Skeletons"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().ends_with(".skeleton.rsrc"))
            .collect();
        files.sort();
        files
    }

    fn load(name: &str) -> SkeletonDefinition {
        open(original_data_dir().join(format!("Skeletons/{name}.skeleton.rsrc")))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    #[test]
    fn parses_every_original_skeleton() {
        let files = skeleton_files();
        assert_eq!(files.len(), 24, "one per SKELETON_TYPE_*");
        for path in files {
            let skeleton = open(&path).unwrap_or_else(|e| panic!("{e:?}"));
            let name = path.display();
            assert!(!skeleton.bones.is_empty(), "{name}");
            assert!(!skeleton.animations.is_empty(), "{name}");
            for anim in &skeleton.animations {
                assert_eq!(anim.keyframes.len(), skeleton.bones.len(), "{name}");
            }
        }
    }

    #[test]
    fn bone_hierarchy_is_a_tree_rooted_at_bone_0() {
        for path in skeleton_files() {
            let skeleton = open(&path).unwrap();
            assert_eq!(skeleton.bones[0].parent, None);
            // Walking the children from the root, as UpdateSkinnedGeometry
            // does, must reach every bone exactly once.
            let mut seen = vec![false; skeleton.bones.len()];
            let mut stack = vec![0];
            while let Some(bone) = stack.pop() {
                assert!(!seen[bone], "{}: bone {bone} reached twice", path.display());
                seen[bone] = true;
                stack.extend(skeleton.children(bone));
            }
            assert!(seen.iter().all(|&s| s), "{}", path.display());
        }
    }

    #[test]
    fn every_decomposed_point_belongs_to_a_bone() {
        for path in skeleton_files() {
            let skeleton = open(&path).unwrap();
            let mut owners = vec![0; skeleton.relative_points.len()];
            // DoodleBug, Mosquito and WaterBug list a few points on more than
            // one bone, so only check that none is left out.
            for bone in &skeleton.bones {
                for &p in &bone.point_indices {
                    owners[usize::from(p)] += 1;
                }
            }
            assert!(owners.iter().all(|&n| n >= 1), "{}", path.display());
        }
    }

    #[test]
    fn keyframes_and_events_are_in_time_order() {
        for path in skeleton_files() {
            let skeleton = open(&path).unwrap();
            for anim in &skeleton.animations {
                let what = format!("{} {:?}", path.display(), anim.name);
                for kfs in &anim.keyframes {
                    assert!(kfs.is_sorted_by(|a, b| a.tick < b.tick), "{what}");
                }
                // UpdateSkeletonAnimation stops scanning at the first event
                // in the future, so events must be sorted.
                assert!(anim.events.is_sorted_by_key(|e| e.time), "{what}");
            }
        }
    }

    #[test]
    fn data_fits_the_original_fixed_size_arrays() {
        // MAX_JOINTS, MAX_ANIMS, MAX_KEYFRAMES and MAX_ANIM_EVENTS in structs.h
        // and skeletonanim.h.
        for path in skeleton_files() {
            let skeleton = open(&path).unwrap();
            assert!(skeleton.bones.len() <= 20);
            assert!(skeleton.animations.len() <= 25);
            for anim in &skeleton.animations {
                assert!(anim.events.len() <= 30);
                assert!(anim.keyframes.iter().all(|k| k.len() <= 15));
            }
        }
    }

    #[test]
    fn animation_counts_match_the_headers() {
        // PLAYER_ANIM_* in myguy.h; ANT_ANIM_*, PONDFISH_ANIM_*, FIREANT_ANIM_* in enemy.h.
        assert_eq!(load("DoodleBug").animations.len(), 21);
        assert_eq!(load("Ant").animations.len(), 10);
        assert_eq!(load("PondFish").animations.len(), 3);
        assert_eq!(load("WingedFireAnt").animations.len(), 8);
    }

    #[test]
    fn ant_details() {
        let ant = load("Ant");
        assert_eq!(ant.bones.len(), 14);
        assert_eq!(ant.bones[0].name, "Pelvis");
        assert_eq!(ant.bones[2].name, "RightShoulder");
        assert_eq!(ant.bones[4].parent, Some(2));

        // ANT_ANIM_THROWSPEAR: Enemy_Ant.c waits for Flag[0] to throw.
        let throw = &ant.animations[2];
        assert_eq!(throw.name, "ThrowSpear");
        assert_eq!(
            throw.events,
            [
                AnimEvent {
                    time: 12,
                    kind: AnimEventKind::SetFlag { flag: 0 }
                },
                AnimEvent {
                    time: 28,
                    kind: AnimEventKind::Stop
                },
            ]
        );
        assert_eq!(throw.max_tick(), 28);
        assert!(
            throw
                .keyframes
                .iter()
                .flatten()
                .any(|k| k.acceleration == AccelerationMode::EaseIn)
        );
    }

    #[test]
    fn player_kick_plays_the_kick_sound() {
        // PLAYER_ANIM_KICK; sound 0 is EFFECT_KICK in UpdateSkeletonAnimation.
        let kick = &load("DoodleBug").animations[4];
        assert_eq!(kick.name, "Kick");
        assert!(
            kick.events
                .iter()
                .any(|e| e.kind == AnimEventKind::PlaySound { sound: 0 })
        );
    }

    #[test]
    fn pond_fish_head_joint() {
        // PONDFISH_JOINT_HEAD in enemy.h.
        assert_eq!(load("PondFish").bones[4].name, "Head");
    }

    #[test]
    fn rejects_missing_resources() {
        assert!(parse(&ResourceFork::default()).is_err());
    }
}
