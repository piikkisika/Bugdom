//! The game's levels and the per-level-type settings: which files a level
//! loads, and its fog, lights and terrain options.
//!
//! Port of the level tables in original/src/System/Main.c (`gLevelTable` and
//! the `gLevel*` arrays used by `InitArea`) and the file choices in
//! `LoadLevelArt` (original/src/System/File.c).

use bevy::prelude::*;

use crate::skeleton::SkeletonType;

/// Number of levels in a game (`NUM_LEVELS`).
pub const NUM_LEVELS: usize = 10;

/// Camera far plane for most level types (`YON_DISTANCE`), in world units.
pub const YON_DISTANCE: f32 = 2500.0;
/// Camera near plane (`HITHER_DISTANCE`), in world units.
pub const HITHER_DISTANCE: f32 = 20.0;
/// The game camera's vertical field of view (`viewDef.camera.fov` in `InitArea`).
pub const CAMERA_FOV: f32 = 1.1;

/// Ambient light brightness, the same in every level
/// (`viewDef.lights.ambientBrightness` in `InitArea`).
pub const AMBIENT_BRIGHTNESS: f32 = 0.2;
/// Brightness of the two fill lights, the same in every level. `InitArea`
/// sets the second one to 0.8 for levels with a ceiling and then overwrites
/// it with 0.5, so 0.5 is what every level uses.
pub const FILL_BRIGHTNESS: [f32; 2] = [1.1, 0.5];

/// The game's level number, 0 to 9 (`gRealLevel`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deref)]
pub struct CurrentLevel(pub usize);

impl CurrentLevel {
    pub fn def(self) -> &'static LevelDef {
        &LEVELS[self.0.min(NUM_LEVELS - 1)]
    }
}

/// The six kinds of level, each with its own scenery and settings
/// (`LEVEL_TYPE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LevelType {
    Lawn,
    Pond,
    Forest,
    Hive,
    Night,
    AntHill,
}

/// One entry of `gLevelTable`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelDef {
    pub name: &'static str,
    pub level_type: LevelType,
    /// Which of the level type's areas this is (`areaNum`).
    pub area: u8,
    pub is_boss_level: bool,
    /// Terrain file, relative to the data directory.
    pub terrain: &'static str,
}

impl LevelDef {
    pub fn settings(&self) -> &'static LevelTypeSettings {
        self.level_type.settings()
    }

    /// Every skeleton the level loads: the global ones, the level type's,
    /// and the king ant on its own level (`LoadLevelArt`).
    pub fn skeletons(&self) -> Vec<SkeletonType> {
        let king = (self.level_type == LevelType::AntHill && self.area == ANT_KING_AREA)
            .then_some(SkeletonType::KingAnt);
        GLOBAL_SKELETONS
            .iter()
            .copied()
            .chain(king)
            .chain(self.settings().skeletons.iter().copied())
            .collect()
    }
}

/// The ant hill area of the king ant's level. `LoadLevelArt` loads the king
/// ant for `LEVEL_NUM_ANTKING`, the only level in this area.
const ANT_KING_AREA: u8 = 1;

/// `gLevelTable`, with the terrain each level loads in `LoadLevelArt`.
pub const LEVELS: [LevelDef; NUM_LEVELS] = [
    level(
        "Training",
        LevelType::Lawn,
        0,
        false,
        "Terrain/Training.ter.rsrc",
    ),
    level("Lawn", LevelType::Lawn, 1, false, "Terrain/Lawn.ter.rsrc"),
    level("Pond", LevelType::Pond, 0, false, "Terrain/Pond.ter.rsrc"),
    level(
        "Beach",
        LevelType::Forest,
        0,
        false,
        "Terrain/Beach.ter.rsrc",
    ),
    level(
        "Dragonfly Attack",
        LevelType::Forest,
        1,
        true,
        "Terrain/Flight.ter.rsrc",
    ),
    level(
        "Bee Hive",
        LevelType::Hive,
        0,
        false,
        "Terrain/BeeHive.ter.rsrc",
    ),
    level(
        "Queen Bee",
        LevelType::Hive,
        1,
        true,
        "Terrain/QueenBee.ter.rsrc",
    ),
    level(
        "Night",
        LevelType::Night,
        0,
        false,
        "Terrain/Night.ter.rsrc",
    ),
    level(
        "Ant Hill",
        LevelType::AntHill,
        0,
        false,
        "Terrain/AntHill.ter.rsrc",
    ),
    level(
        "Ant King",
        LevelType::AntHill,
        1,
        true,
        "Terrain/AntKing.ter.rsrc",
    ),
];

const fn level(
    name: &'static str,
    level_type: LevelType,
    area: u8,
    is_boss_level: bool,
    terrain: &'static str,
) -> LevelDef {
    LevelDef {
        name,
        level_type,
        area,
        is_boss_level,
        terrain,
    }
}

/// The per-level-type tables of `Main.c`, plus the model files from
/// `LoadLevelArt`. Colours are the original's display values (sRGB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelTypeSettings {
    /// Level-specific model files, relative to the data directory
    /// (`MODEL_GROUP_LEVELSPECIFIC` and `MODEL_GROUP_LEVELSPECIFIC2`).
    pub models: &'static [&'static str],
    /// Level-specific skeletons (`LoadASkeleton` in `LoadLevelArt`).
    pub skeletons: &'static [SkeletonType],
    /// `gLevelHasCyc`
    pub has_cyclorama: bool,
    /// `gLevelHasCeiling`
    pub has_ceiling: bool,
    /// `gLevelSuperTileActiveRange`: how many supertiles around the camera's
    /// focus keep their items alive.
    pub supertile_active_range: u8,
    /// `gLevelFogStart` and `gLevelFogEnd`, as fractions of the far plane.
    pub fog_start: f32,
    pub fog_end: f32,
    /// `gLevelAutoFadeStart`, in world units; 0 if objects don't fade out.
    pub auto_fade_start: f32,
    /// `gLevelHasLensFlare`
    pub has_lens_flare: bool,
    /// `gLensFlareVector`: the direction of the main fill light (not normalised).
    pub light_direction: Vec3,
    /// `gLevelLightColors`: ambient, fill 0, fill 1.
    pub light_colors: [[f32; 3]; 3],
    /// `gLevelFogColor`, also the clear colour without a cyclorama.
    pub fog_color: [f32; 3],
    /// `gLevelClearColorWithCyc`
    pub clear_color_with_cyclorama: [f32; 3],
}

impl LevelTypeSettings {
    /// The camera's far plane (`gCurrentYon` in `InitArea`).
    pub fn yon(&self) -> f32 {
        if self.supertile_active_range == 5 {
            YON_DISTANCE + 1700.0
        } else {
            YON_DISTANCE
        }
    }

    /// The two fill lights' directions, normalised, in the direction the
    /// light travels (`gLightDirection1` and `gLightDirection2` in `InitArea`).
    pub fn fill_directions(&self) -> [Vec3; 2] {
        let second = if self.has_ceiling {
            Vec3::new(-0.8, 1.0, -0.2)
        } else {
            Vec3::new(-0.2, -0.7, -0.1)
        };
        [
            self.light_direction.normalize_or(Vec3::NEG_Y),
            second.normalize(),
        ]
    }

    /// The colour the camera clears to, which shows where nothing is drawn.
    pub fn clear_color(&self) -> [f32; 3] {
        if self.has_cyclorama {
            self.clear_color_with_cyclorama
        } else {
            self.fog_color
        }
    }
}

impl LevelType {
    pub fn settings(self) -> &'static LevelTypeSettings {
        match self {
            Self::Lawn => &LAWN,
            Self::Pond => &POND,
            Self::Forest => &FOREST,
            Self::Hive => &HIVE,
            Self::Night => &NIGHT,
            Self::AntHill => &ANTHILL,
        }
    }
}

const LAWN: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/Lawn_Models1.3dmf", "Models/Lawn_Models2.3dmf"],
    skeletons: &[
        SkeletonType::BoxerFly,
        SkeletonType::Slug,
        SkeletonType::Ant,
    ],
    has_cyclorama: true,
    has_ceiling: false,
    supertile_active_range: 5,
    fog_start: 0.5,
    fog_end: 0.9,
    auto_fade_start: YON_DISTANCE + 400.0,
    has_lens_flare: true,
    light_direction: Vec3::new(0.4, -0.35, 1.0),
    light_colors: [[1.0, 1.0, 0.9], [1.0, 1.0, 0.6], [1.0, 1.0, 1.0]],
    fog_color: [0.05, 0.25, 0.05],
    clear_color_with_cyclorama: [0.352, 0.380, 1.0],
};

const POND: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/Pond_Models.3dmf"],
    skeletons: &[
        SkeletonType::Mosquito,
        SkeletonType::WaterBug,
        SkeletonType::PondFish,
        SkeletonType::Skippy,
        SkeletonType::Slug,
    ],
    has_cyclorama: false,
    has_ceiling: false,
    supertile_active_range: 4,
    fog_start: 0.4,
    fog_end: 1.0,
    auto_fade_start: 0.0,
    has_lens_flare: true,
    light_direction: Vec3::new(0.4, -0.45, 1.0),
    light_colors: [[1.0, 1.0, 0.9], [1.0, 1.0, 0.6], [1.0, 1.0, 1.0]],
    fog_color: [0.9, 0.9, 0.85],
    clear_color_with_cyclorama: [0.9, 0.9, 0.85],
};

const FOREST: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/Forest_Models.3dmf"],
    skeletons: &[
        SkeletonType::DragonFly,
        SkeletonType::Foot,
        SkeletonType::Spider,
        SkeletonType::Caterpiller,
        SkeletonType::Bat,
        SkeletonType::FlyingBee,
        SkeletonType::Ant,
    ],
    has_cyclorama: true,
    has_ceiling: false,
    supertile_active_range: 5,
    fog_start: 0.6,
    fog_end: 0.85,
    auto_fade_start: 0.0,
    has_lens_flare: true,
    light_direction: Vec3::new(0.4, -0.15, 1.0),
    light_colors: [[1.0, 0.6, 0.3], [1.0, 0.8, 0.3], [1.0, 0.9, 0.3]],
    fog_color: [1.0, 0.29, 0.063],
    clear_color_with_cyclorama: [1.0, 0.29, 0.063],
};

const HIVE: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/BeeHive_Models.3dmf"],
    skeletons: &[
        SkeletonType::Larva,
        SkeletonType::FlyingBee,
        SkeletonType::WorkerBee,
        SkeletonType::QueenBee,
    ],
    has_cyclorama: false,
    has_ceiling: true,
    supertile_active_range: 4,
    fog_start: 0.85,
    fog_end: 1.0,
    auto_fade_start: 0.0,
    has_lens_flare: false,
    light_direction: Vec3::new(0.4, -0.35, 1.0),
    light_colors: [[1.0, 1.0, 0.8], [1.0, 1.0, 0.7], [1.0, 1.0, 0.9]],
    fog_color: [0.7, 0.6, 0.4],
    clear_color_with_cyclorama: [0.7, 0.6, 0.4],
};

const NIGHT: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/Night_Models.3dmf"],
    skeletons: &[
        SkeletonType::FireAnt,
        SkeletonType::FireFly,
        SkeletonType::Caterpiller,
        SkeletonType::Slug,
        SkeletonType::Roach,
        SkeletonType::Ant,
    ],
    has_cyclorama: true,
    has_ceiling: false,
    supertile_active_range: 4,
    fog_start: 0.4,
    fog_end: 0.9,
    auto_fade_start: YON_DISTANCE - 250.0,
    has_lens_flare: true,
    light_direction: Vec3::new(0.4, -0.35, 1.0),
    light_colors: [[0.5, 0.5, 0.5], [0.8, 1.0, 0.8], [0.6, 0.8, 0.7]],
    fog_color: [0.02, 0.02, 0.08],
    clear_color_with_cyclorama: [0.02, 0.02, 0.08],
};

const ANTHILL: LevelTypeSettings = LevelTypeSettings {
    models: &["Models/AntHill_Models.3dmf"],
    skeletons: &[
        SkeletonType::Slug,
        SkeletonType::Ant,
        SkeletonType::FireAnt,
        SkeletonType::RootSwing,
        SkeletonType::Roach,
    ],
    has_cyclorama: false,
    has_ceiling: true,
    supertile_active_range: 4,
    fog_start: 0.65,
    fog_end: 1.0,
    auto_fade_start: 0.0,
    has_lens_flare: false,
    light_direction: Vec3::new(0.4, -0.35, 1.0),
    light_colors: [[0.5, 0.5, 0.6], [0.7, 0.7, 0.8], [1.0, 1.0, 1.0]],
    fog_color: [0.15, 0.07, 0.15],
    clear_color_with_cyclorama: [0.15, 0.07, 0.15],
};

/// Skeletons every level loads (`LoadLevelArt`).
pub const GLOBAL_SKELETONS: [SkeletonType; 3] =
    [SkeletonType::Me, SkeletonType::LadyBug, SkeletonType::Buddy];

/// Model files every level loads (`LoadLevelArt`).
pub const GLOBAL_MODELS: [&str; 2] = ["Models/Global_Models1.3dmf", "Models/Global_Models2.3dmf"];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use bugdom_formats::{skeleton, skin, tdmf};

    use super::*;

    /// Every skeleton a level loads exists, parses, and binds to its
    /// geometry, as the skeleton asset loader does.
    #[test]
    fn every_level_skeleton_loads() {
        let used: HashSet<SkeletonType> = LEVELS.iter().flat_map(LevelDef::skeletons).collect();
        let dir = bugdom_formats::original_data_dir();
        for kind in used {
            let path = dir.join(kind.path());
            let definition = skeleton::open(&path)
                .unwrap_or_else(|e| panic!("{kind:?}: {}: {e}", path.display()));
            let model_path = dir.join(format!("Skeletons/{}.3dmf", kind.file_name()));
            let model = tdmf::open(&model_path)
                .unwrap_or_else(|e| panic!("{kind:?}: {}: {e}", model_path.display()));
            skin::bind(&definition, &model).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        }
    }

    #[test]
    fn levels_load_their_skeletons_once() {
        for def in &LEVELS {
            let skeletons = def.skeletons();
            let unique: HashSet<_> = skeletons.iter().collect();
            assert_eq!(unique.len(), skeletons.len(), "{}", def.name);
            assert!(skeletons.contains(&SkeletonType::Me), "{}", def.name);
            let king = skeletons.contains(&SkeletonType::KingAnt);
            assert_eq!(king, def.name == "Ant King", "{}", def.name);
        }
    }

    /// Only the skeletons no level loads are missing from the levels.
    #[test]
    fn every_skeleton_type_is_used() {
        let used: HashSet<SkeletonType> = LEVELS.iter().flat_map(LevelDef::skeletons).collect();
        let unused: Vec<_> = SkeletonType::ALL
            .into_iter()
            .filter(|kind| !used.contains(kind))
            .collect();
        assert_eq!(unused, []);
    }
}
