//! The player: spawning, the bug and ball forms' controllers, and the bug's
//! animation.
//!
//! Port of original/src/Player (MyGuy.c, Player_Bug.c, Player_Ball.c,
//! Player_Control.c).

mod animation;
mod ball;
mod bug;
mod movement;
mod tuning;

use std::f32::consts::TAU;

use avian3d::prelude::{LayerMask, TransformInterpolation};
use bevy::prelude::*;

pub use ball::{
    BallSpin, BallTime, Nitro, PLAYER_BALL_FOOT_OFFSET, PLAYER_BALL_HEAD_OFFSET,
    has_headroom_to_unroll,
};
pub use bug::BugState;
pub use tuning::{BallTuning, BugTuning, FormMotion, PlayerTuning};

use crate::collision::{
    CollisionBox, CollisionCandidates, CollisionKind, CollisionSystems, SolidSides, solid_object,
};
use crate::input::{ControlInput, ControlSettings, LocalControls};
use crate::objects::{ModelSpawner, attach_shadow};
use crate::physics::{GroundContact, PreviousPosition, Velocity};
use crate::skeleton::{Skeleton, SkeletonSystems};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{PlayerStart, TerrainMap, TerrainSystems};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerTuning>()
            .add_systems(
                OnEnter(AppState::InGame),
                spawn_player
                    .in_set(PlayerSystems::Spawn)
                    .after(TerrainSystems::Spawn),
            )
            // As in the original, a change of form (and the animation it
            // starts) comes before animations advance and anything moves.
            .add_systems(
                FixedUpdate,
                (ball::check_player_morph, animation::animate_bug)
                    .chain()
                    .in_set(PlayerSystems::Morph)
                    .before(SkeletonSystems::Advance)
                    .before(CollisionSystems::Gather)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                FixedUpdate,
                (
                    bug::move_bug,
                    ball::move_ball,
                    animation::animate_bug,
                    pose_player_model,
                )
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
    /// Changes the player's form when asked to, each fixed tick.
    Morph,
    /// Moves the player each fixed tick.
    Move,
}

/// The player character. Its translation is the bug's feet, or the ball's
/// centre; its rotation is only its heading.
#[derive(Component, Debug, Clone, Copy, Default)]
#[require(
    PlayerForm,
    BugState,
    Velocity,
    GroundContact,
    PreviousPosition,
    CollisionCandidates,
    PlayerSpeed,
    PlayerSteering,
    BallSpin,
    Nitro,
    BallTime,
    ControlInput,
    ControlSettings,
    PlayerToCameraAngle,
    RespawnPoint
)]
pub struct Player;

/// Which form the player is in (`gPlayerMode`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PlayerForm {
    #[default]
    Bug,
    Ball,
}

impl PlayerForm {
    /// The form's collision box (`SetObjectCollisionBounds` in
    /// `InitPlayer_Bug` and `InitPlayer_Ball`). Both have the same
    /// footprint and the bottom at the floor, so changing form can't drop
    /// the player through things; the ball is shorter.
    pub const fn collision_box(self) -> CollisionBox {
        match self {
            Self::Bug => CollisionBox::new(PLAYER_BUG_HEAD_OFFSET, 0.0, -42.0, 42.0, 42.0, -42.0),
            Self::Ball => CollisionBox::new(
                PLAYER_BALL_HEAD_OFFSET,
                -PLAYER_BALL_FOOT_OFFSET,
                -42.0,
                42.0,
                42.0,
                -42.0,
            ),
        }
    }
}

/// The child entity that shows the player's skeleton, which the ball turns
/// about its centre.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerModel(pub Entity);

/// The player's horizontal speed as the controls left it, before collision
/// (`ObjNode::Speed`), in units per second. The walk animation's speed, the
/// turning rate and the ball's spin follow it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerSpeed(pub f32);

/// The steering acceleration the controls set, in the original's units
/// (`ObjNode::AccelVector`). It persists between ticks: some states move
/// before they read the controls, and so use the previous tick's value.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerSteering(pub Vec2);

/// Size of the bug model (`PLAYER_BUG_SCALE`). The ball keeps the bug's
/// geometry at this size.
const PLAYER_BUG_SCALE: f32 = 1.7;
/// Height of the bug's head above its origin (`PLAYER_BUG_HEADOFFSET`). Its
/// feet are at the origin (`PLAYER_BUG_FOOTOFFSET` is 0).
pub const PLAYER_BUG_HEAD_OFFSET: f32 = 180.0;
/// The player's radius against fences, the same for both forms so that
/// changing form near a fence can't push it through (`PLAYER_RADIUS`).
pub const PLAYER_RADIUS: f32 = 60.0;
/// Size of the player's shadow (`AttachShadowToObject` in `InitPlayer_Bug`).
/// The ball keeps the bug's shadow.
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

/// The angle around the player of the camera that follows it, which makes
/// the mouse controls camera-relative (`gPlayerToCameraAngle`). The camera
/// updates it after the player moves.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerToCameraAngle(pub f32);

/// Where the player restarts after dying: the start, or the best
/// checkpoint it has tagged (`gBestCheckPoint`, `gMostRecentCheckPointCoord`
/// and `gCheckPointRot`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct RespawnPoint {
    /// The highest checkpoint number reached, if any.
    pub checkpoint: Option<u8>,
    pub position: Vec3,
    pub yaw: f32,
}

/// Puts the bug on the floor at the level's start. Ball time and the
/// other inventory start full for now; carrying them over between levels
/// arrives with the game flow.
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
    let player = commands
        .spawn((
            Name::new("Player"),
            Player,
            LocalControls,
            // Rounded down to a quarter turn (`InitPlayerAtStartOfLevel`).
            RespawnPoint {
                checkpoint: None,
                position,
                yaw: f32::from(start.aim / 2) * (TAU / 4.0),
            },
            Transform::from_translation(position).with_rotation(Quat::from_rotation_y(start.yaw())),
            // The model is a child, which needs visibility to inherit.
            Visibility::default(),
            TransformInterpolation,
            PreviousPosition(position),
            // Others only touch the player; it decides what happens itself.
            solid_object(
                vec![PlayerForm::Bug.collision_box()],
                CollisionKind::Player,
                SolidSides::TOUCHABLE,
            ),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    let model = commands
        .spawn((
            Name::new("Player model"),
            Skeleton(level_assets.player_skeleton.clone()),
            Transform::from_scale(Vec3::splat(PLAYER_BUG_SCALE)),
            TransformInterpolation,
            ChildOf(player),
        ))
        .id();
    commands.entity(player).insert(PlayerModel(model));
    attach_shadow(
        &mut commands,
        &mut models,
        player,
        Vec2::splat(PLAYER_SHADOW_SCALE),
        true,
    );
}

/// Places the player's model: the bug stands on the player's origin; the
/// ball's frozen roll-up pose sits below its centre and rolls about it.
///
/// Port of the ball's transform in `UpdatePlayer_Ball`
/// (original/src/Player/Player_Ball.c), whose mesh `InitPlayer_Ball` moves
/// down by `PLAYER_BALL_FOOTOFFSET` and which turns about x, then y.
fn pose_player_model(
    players: Query<(&PlayerForm, &BallSpin, &PlayerModel)>,
    mut models: Query<&mut Transform, Without<PlayerForm>>,
) {
    for (form, spin, model) in &players {
        let Ok(mut transform) = models.get_mut(model.0) else {
            continue;
        };
        let (rotation, offset) = match form {
            PlayerForm::Bug => (Quat::IDENTITY, Vec3::ZERO),
            PlayerForm::Ball => {
                let roll = Quat::from_rotation_x(spin.angle);
                (roll, roll * Vec3::new(0.0, -PLAYER_BALL_FOOT_OFFSET, 0.0))
            }
        };
        transform.set_if_neq(Transform {
            translation: offset,
            rotation,
            scale: Vec3::splat(PLAYER_BUG_SCALE),
        });
    }
}
