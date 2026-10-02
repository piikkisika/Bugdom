//! The player: spawning, the bug form's controller and its animation.
//!
//! Port of original/src/Player (MyGuy.c, Player_Bug.c, Player_Control.c).

mod animation;
mod bug;

use avian3d::prelude::{LayerMask, TransformInterpolation};
use bevy::prelude::*;

pub use bug::{BugState, PlayerTuning};

use crate::collision::{
    CollisionBox, CollisionCandidates, CollisionKind, CollisionSystems, SolidSides, solid_object,
};
use crate::objects::{ModelSpawner, attach_shadow};
use crate::physics::{GroundContact, PreviousPosition, Velocity};
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
                    .after(CollisionSystems::Gather)
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
#[require(Velocity, GroundContact, PreviousPosition, CollisionCandidates)]
pub struct Player;

/// Size of the bug model (`PLAYER_BUG_SCALE`).
const PLAYER_BUG_SCALE: f32 = 1.7;
/// Height of the bug's head above its origin (`PLAYER_BUG_HEADOFFSET`). Its
/// feet are at the origin (`PLAYER_BUG_FOOTOFFSET` is 0).
pub const PLAYER_BUG_HEAD_OFFSET: f32 = 180.0;
/// The player's collision box, the same for the bug and the ball so that
/// changing form can't drop it through things (`SetObjectCollisionBounds`
/// in `InitPlayer_Bug`).
pub const PLAYER_BOX: CollisionBox =
    CollisionBox::new(PLAYER_BUG_HEAD_OFFSET, 0.0, -42.0, 42.0, 42.0, -42.0);
/// The player's radius against fences (`PLAYER_RADIUS`).
pub const PLAYER_RADIUS: f32 = 60.0;
/// Size of the player's shadow (`AttachShadowToObject` in `InitPlayer_Bug`).
const PLAYER_SHADOW_SCALE: f32 = 4.0;

/// What the player bumps into (`PLAYER_COLLISION_CTYPE`).
pub fn player_collision_mask() -> LayerMask {
    LayerMask::from([
        CollisionKind::Misc,
        CollisionKind::Enemy,
        CollisionKind::HurtMe,
        CollisionKind::Trigger,
        CollisionKind::Liquid,
        CollisionKind::AutoTarget,
        CollisionKind::Viscous,
    ])
}

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
    mut models: ModelSpawner,
) {
    let (x, z) = (start.position.x, start.position.y);
    let position = Vec3::new(x, map.floor_height(x, z), z);
    let player = commands.spawn((
        Name::new("Player"),
        Player,
        BugState::Stand,
        Skeleton(level_assets.player_skeleton.clone()),
        Transform::from_translation(position)
            .with_rotation(Quat::from_rotation_y(start.yaw()))
            .with_scale(Vec3::splat(PLAYER_BUG_SCALE)),
        TransformInterpolation,
        PreviousPosition(position),
        // Others only touch the player; it decides what happens itself.
        // The collider grows with the model's scale, which only makes the
        // broad phase find it from a little further away.
        solid_object(
            vec![PLAYER_BOX],
            CollisionKind::Player,
            SolidSides::TOUCHABLE,
        ),
        DespawnOnExit(AppState::InGame),
    ));
    let player = player.id();
    attach_shadow(
        &mut commands,
        &mut models,
        player,
        Vec2::splat(PLAYER_SHADOW_SCALE),
        true,
    );
}
