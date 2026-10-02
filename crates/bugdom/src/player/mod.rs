//! The player: spawning, the bug form's controller and its animation.
//!
//! Port of original/src/Player (MyGuy.c, Player_Bug.c, Player_Control.c).

mod animation;
mod bug;

use avian3d::prelude::TransformInterpolation;
use bevy::prelude::*;

pub use bug::{BugState, PlayerTuning};

use crate::physics::{GroundContact, Velocity};
use crate::skeleton::{Skeleton, SkeletonSystems};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{PlayerStart, TerrainMap, TerrainSystems};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerTuning>()
            .init_resource::<PlayerToCameraAngle>()
            .add_systems(
                OnEnter(AppState::InGame),
                spawn_player
                    .in_set(PlayerSystems::Spawn)
                    .after(TerrainSystems::Spawn),
            )
            .add_systems(
                FixedUpdate,
                (bug::move_bug, animation::animate_bug)
                    .chain()
                    .in_set(PlayerSystems::Move)
                    .after(SkeletonSystems::Advance)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlayerSystems {
    /// Spawns the player when the level starts.
    Spawn,
    /// Moves the player each fixed tick.
    Move,
}

/// The player character.
#[derive(Component, Debug, Clone, Copy, Default)]
#[require(Velocity, GroundContact)]
pub struct Player;

/// Size of the bug model (`PLAYER_BUG_SCALE`).
const PLAYER_BUG_SCALE: f32 = 1.7;
/// Height of the bug's head above its origin (`PLAYER_BUG_HEADOFFSET`). Its
/// feet are at the origin (`PLAYER_BUG_FOOTOFFSET` is 0).
pub const PLAYER_BUG_HEAD_OFFSET: f32 = 180.0;
/// The bug's skeleton file (`SKELETON_TYPE_ME`).
pub const PLAYER_SKELETON: &str = "Skeletons/DoodleBug.skeleton.rsrc";

/// The angle of the camera around the player, which makes the mouse
/// controls camera-relative (`gPlayerToCameraAngle`). The camera updates it
/// after the player moves.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerToCameraAngle(pub f32);

/// Puts the bug on the floor at the level's start.
///
/// Port of `InitPlayerAtStartOfLevel` (original/src/Player/MyGuy.c) and
/// `InitPlayer_Bug` (original/src/Player/Player_Bug.c).
fn spawn_player(
    mut commands: Commands,
    start: Res<PlayerStart>,
    map: Res<TerrainMap>,
    level_assets: Res<LevelAssets>,
) {
    let (x, z) = (start.position.x, start.position.y);
    let position = Vec3::new(x, map.floor_height(x, z), z);
    commands.spawn((
        Name::new("Player"),
        Player,
        BugState::Stand,
        Skeleton(level_assets.player_skeleton.clone()),
        Transform::from_translation(position)
            .with_rotation(Quat::from_rotation_y(start.yaw()))
            .with_scale(Vec3::splat(PLAYER_BUG_SCALE)),
        TransformInterpolation,
        DespawnOnExit(AppState::InGame),
    ));
}
