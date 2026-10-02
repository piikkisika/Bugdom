//! The game's top-level states and level loading.
//!
//! Replaces the original's blocking screen loops (`GameMain`, `PlayGame` and
//! `PlayArea` in original/src/System/Main.c) with Bevy states. Everything a
//! level spawns carries `DespawnOnExit(AppState::InGame)`, which does the work
//! of `CleanupLevel`.

use bevy::prelude::*;

use crate::assets::model::Model;
use crate::assets::original_path;
use crate::assets::skeleton::SkeletonAsset;
use crate::assets::terrain::TerrainAsset;
use crate::fences::FenceKind;
use crate::level::{CurrentLevel, GLOBAL_MODELS, NUM_LEVELS};
use crate::liquids::LiquidKind;
use crate::player::PLAYER_SKELETON;

/// Environment variable that picks the starting level (0 to 9), standing in
/// for the original's level-select cheat until the menus exist.
pub const LEVEL_ENV: &str = "BUGDOM_LEVEL";
/// The level to start on without [`LEVEL_ENV`]: the Lawn, the level the
/// Phase 2 milestone is checked on.
const DEFAULT_LEVEL: usize = 1;

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .insert_resource(CurrentLevel(starting_level()))
            .add_systems(OnEnter(AppState::Loading), load_level_assets)
            .add_systems(Update, finish_loading.run_if(in_state(AppState::Loading)));
    }
}

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Loading the current level's files (`LoadLevelArt`).
    #[default]
    Loading,
    /// Playing a level (`PlayArea`).
    InGame,
}

/// The current level's files (`LoadLevelArt` in original/src/System/File.c).
#[derive(Resource, Debug, Clone)]
pub struct LevelAssets {
    pub terrain: Handle<TerrainAsset>,
    /// Global model files first, then the level type's.
    pub models: Vec<Handle<Model>>,
    pub player_skeleton: Handle<SkeletonAsset>,
    /// The textures of the fence types the level type has (`PrimeFences`).
    pub fence_textures: Vec<(FenceKind, Handle<Image>)>,
    /// The textures of the liquids the level type has (`InitLiquids`).
    pub liquid_textures: Vec<(LiquidKind, Handle<Image>)>,
}

fn starting_level() -> usize {
    match std::env::var(LEVEL_ENV) {
        Ok(value) => match value.parse::<usize>() {
            Ok(level) if level < NUM_LEVELS => level,
            _ => {
                warn!("{LEVEL_ENV}={value} is not a level number from 0 to 9");
                DEFAULT_LEVEL
            }
        },
        Err(_) => DEFAULT_LEVEL,
    }
}

fn load_level_assets(mut commands: Commands, assets: Res<AssetServer>, level: Res<CurrentLevel>) {
    let def = level.def();
    info!("Loading level {} ({})", **level, def.name);
    let models = GLOBAL_MODELS
        .iter()
        .chain(def.settings().models)
        .map(|path| assets.load(original_path(path)))
        .collect();
    commands.insert_resource(LevelAssets {
        terrain: assets.load(original_path(def.terrain)),
        models,
        player_skeleton: assets.load(original_path(PLAYER_SKELETON)),
        fence_textures: FenceKind::on_level(def.level_type)
            .iter()
            .map(|kind| (*kind, assets.load(original_path(&kind.texture_path()))))
            .collect(),
        liquid_textures: LiquidKind::on_level(def.level_type)
            .iter()
            .map(|kind| {
                let path = kind.texture_path(def.level_type);
                (*kind, assets.load(original_path(&path)))
            })
            .collect(),
    });
}

fn finish_loading(
    level_assets: Res<LevelAssets>,
    assets: Res<AssetServer>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
) {
    let handles = std::iter::once(level_assets.terrain.id().untyped())
        .chain(level_assets.models.iter().map(|h| h.id().untyped()))
        .chain(std::iter::once(level_assets.player_skeleton.id().untyped()))
        .chain(
            level_assets
                .fence_textures
                .iter()
                .map(|(_, h)| h.id().untyped()),
        )
        .chain(
            level_assets
                .liquid_textures
                .iter()
                .map(|(_, h)| h.id().untyped()),
        );
    let mut ready = true;
    for id in handles {
        if let Some(bevy::asset::LoadState::Failed(error)) = assets.get_load_state(id) {
            // A level can't run without its data.
            error!("Failed to load level data: {error}");
            exit.write(AppExit::error());
            return;
        }
        ready &= assets.is_loaded_with_dependencies(id);
    }
    if ready {
        next.set(AppState::InGame);
    }
}
