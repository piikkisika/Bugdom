//! The game's top-level states, the flow between levels, and level loading.
//!
//! Replaces the original's blocking screen loops (`GameMain`, `PlayGame` and
//! `PlayArea` in original/src/System/Main.c) with Bevy states:
//!
//! ```text
//! Boot → Loading → InGame ─┬─ area completed → Loading (next level)
//!   ↑                      └─ last area completed ─┐
//!   └──────────────────────────────────────────────┘
//! ```
//!
//! While in a level, [`GameplayState`] says whether it is playing or paused
//! (see [`crate::pause`]). Everything a level spawns carries
//! `DespawnOnExit(AppState::InGame)`, which does the work of `CleanupLevel`.
//! Phase 4 adds the title screen, the level intro, the bonus screen and the
//! win and lose screens between these states.

use bevy::prelude::*;

use crate::assets::model::Model;
use crate::assets::original_path;
use crate::assets::skeleton::SkeletonAsset;
use crate::assets::terrain::TerrainAsset;
use crate::fences::FenceKind;
use crate::items::AreaCompleted;
use crate::level::{CurrentLevel, GLOBAL_MODELS, NUM_LEVELS};
use crate::liquids::LiquidKind;
use crate::player::PLAYER_SKELETON;

/// Environment variable that picks the starting level (0 to 9), standing in
/// for the original's level-select cheat until the menus exist.
pub const LEVEL_ENV: &str = "BUGDOM_LEVEL";
/// The level to start on without [`LEVEL_ENV`]: the Lawn, the level the
/// Phase 2 milestone is checked on.
const DEFAULT_LEVEL: usize = 1;
/// The last level; completing it wins the game (`LEVEL_NUM_ANTKING`).
const FINAL_LEVEL: usize = NUM_LEVELS - 1;
/// Held down to enable the cheat keys (`SDL_SCANCODE_GRAVE` in
/// `CheckForCheats`).
pub const CHEAT_KEY: KeyCode = KeyCode::Backquote;
/// With [`CHEAT_KEY`], completes the area.
const COMPLETE_AREA_CHEAT: KeyCode = KeyCode::F1;

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .add_sub_state::<GameplayState>()
            .init_resource::<CurrentLevel>()
            .add_systems(OnEnter(AppState::Boot), start_game)
            .add_systems(OnEnter(AppState::Loading), load_level_assets)
            .add_systems(Update, finish_loading.run_if(in_state(AppState::Loading)))
            .add_systems(
                Update,
                (complete_area_cheat, end_area)
                    .chain()
                    .run_if(in_state(GameplayState::Playing)),
            );
    }
}

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Starting a new game (the set-up at the top of `PlayGame`).
    #[default]
    Boot,
    /// Loading the current level's files (`LoadLevelArt`).
    Loading,
    /// Playing a level (`PlayArea`).
    InGame,
}

/// What is happening in a level. A killed player waits out its kill delay
/// on its own entity ([`crate::player::Dying`]), so that each player can
/// die on its own, rather than in a state shared by everyone.
#[derive(SubStates, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[source(AppState = AppState::InGame)]
pub enum GameplayState {
    /// The level runs.
    #[default]
    Playing,
    /// The level is frozen (`gIsGamePaused`).
    Paused,
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

/// Starts a new game at the starting level.
///
/// Port of the set-up in `PlayGame` (original/src/System/Main.c). The level
/// comes from [`LEVEL_ENV`] rather than from the level-select cheat, and
/// without it the game starts on the Lawn rather than the training level
/// until the menus exist. `InitInventoryForGame` arrives with the inventory
/// in Phase 4.
fn start_game(mut level: ResMut<CurrentLevel>, mut next: ResMut<NextState<AppState>>) {
    *level = CurrentLevel(starting_level());
    next.set(AppState::Loading);
}

/// Completes the area when the cheat keys are pressed.
///
/// Port of the `F1` cheat in `CheckForCheats` (original/src/System/Main.c).
/// The other cheats (shield, health, ball time) arrive with what they fill
/// up. Unlike the original's debug builds, the cheat key must always be
/// held, because `F1` alone toggles the debug fly camera.
fn complete_area_cheat(keys: Res<ButtonInput<KeyCode>>, mut completed: ResMut<AreaCompleted>) {
    if keys.pressed(CHEAT_KEY) && keys.just_pressed(COMPLETE_AREA_CHEAT) {
        **completed = true;
    }
}

/// Leaves a completed area for the next level, or starts again once the
/// last one is completed.
///
/// Port of the area-completed check in `PlayArea` and of the level loop in
/// `PlayGame` (original/src/System/Main.c). The fade, the bonus screen and
/// the win screen arrive in Phase 4; until then winning goes straight back
/// to [`AppState::Boot`], as the original returns to the title screen.
fn end_area(
    completed: Res<AreaCompleted>,
    mut level: ResMut<CurrentLevel>,
    mut next: ResMut<NextState<AppState>>,
) {
    if !**completed {
        return;
    }
    match next_level(*level) {
        Some(next_level) => {
            *level = next_level;
            next.set(AppState::Loading);
        }
        None => {
            info!("Won the game");
            next.set(AppState::Boot);
        }
    }
}

/// The level after `level`, or `None` after the last one.
fn next_level(level: CurrentLevel) -> Option<CurrentLevel> {
    (*level < FINAL_LEVEL).then(|| CurrentLevel(*level + 1))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_in_order_until_the_ant_king() {
        assert_eq!(next_level(CurrentLevel(0)), Some(CurrentLevel(1)));
        assert_eq!(next_level(CurrentLevel(8)), Some(CurrentLevel(9)));
        assert_eq!(next_level(CurrentLevel(FINAL_LEVEL)), None);
    }
}
