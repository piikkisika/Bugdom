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
use crate::skeleton::{Skeleton, SkeletonSystems, SkeletonType};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{PlayerStart, TerrainMap, TerrainSystems};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerTuning>()
            .add_message::<PlayerRespawned>()
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
            )
            .add_systems(
                FixedUpdate,
                respawn_dead_players
                    .in_set(PlayerSystems::Respawn)
                    .after(PlayerSystems::Move)
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
    /// Starts killed players again once their kill delay is over.
    Respawn,
}

/// A killed player, waiting to start again (`gPlayerGotKilledFlag`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Dying {
    /// Seconds until it starts again (`killDelay` in `PlayArea`).
    pub timer: f32,
}

/// Sent when a killed player starts again at its [`RespawnPoint`], so that
/// its camera can jump back behind it.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerRespawned(pub Entity);

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
    // Every level loads the bug's skeleton.
    let Some(skeleton) = level_assets.skeleton(SkeletonType::Me) else {
        error!("The level has no player skeleton");
        return;
    };
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
            Skeleton(skeleton),
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

/// Starts each killed player again at its [`RespawnPoint`] once its kill
/// delay is over.
///
/// Port of the kill timer in `PlayArea` and of `DoDeathReset`
/// (original/src/System/Main.c) and `ResetPlayer`
/// (original/src/Player/MyGuy.c). Losing a life, the game over, the fade,
/// health and the invincibility after starting again arrive with the
/// infobar and the game flow.
fn respawn_dead_players(
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut commands: Commands,
    mut respawned: MessageWriter<PlayerRespawned>,
    mut players: Query<(movement::PlayerData, &RespawnPoint, &mut Dying), With<Player>>,
) {
    for (mut player, respawn, mut dying) in &mut players {
        dying.timer -= time.delta_secs();
        if dying.timer >= 0.0 {
            continue;
        }
        let point = respawn.position;
        if *player.form == PlayerForm::Ball {
            // A new bug, as when unrolling: it keeps the ball's heading and
            // starts below the checkpoint's height by the ball's offset.
            player.set_form(PlayerForm::Bug);
            player.transform.translation = point - Vec3::Y * PLAYER_BALL_FOOT_OFFSET;
            *player.spin = BallSpin::default();
        } else {
            player.transform.translation =
                Vec3::new(point.x, map.floor_height(point.x, point.z), point.z);
            player.transform.rotation = Quat::from_rotation_y(respawn.yaw);
        }
        *player.state = BugState::Stand;
        player.animated.restart();
        **player.velocity = Vec3::ZERO;
        **player.previous = player.transform.translation;
        // The original leaves the liquid flag for the next collision check
        // to clear; clearing it now only differs for that one tick.
        commands
            .entity(player.entity)
            .remove::<(Dying, crate::liquids::Underwater)>();
        respawned.write(PlayerRespawned(player.entity));
    }
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

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn only_players_whose_kill_delay_is_over_start_again_at_their_checkpoint() {
        let mut world = World::new();
        let map = TerrainMap::load_for_tests("Lawn", false);
        let floor = map.floor_height(12000.0, 15000.0);
        world.insert_resource(map);
        world.init_resource::<Time>();
        world.init_resource::<Messages<PlayerRespawned>>();
        let checkpoint = RespawnPoint {
            checkpoint: Some(1),
            position: Vec3::new(12000.0, floor + 300.0, 15000.0),
            yaw: 1.0,
        };
        let mut spawn = |timer: f32| {
            world
                .spawn((
                    Player,
                    Transform::from_xyz(13000.0, 0.0, 16000.0),
                    solid_object(
                        vec![PlayerForm::Bug.collision_box()],
                        CollisionKind::Player,
                        SolidSides::TOUCHABLE,
                    ),
                    checkpoint,
                    Dying { timer },
                    BugState::Swim,
                ))
                .id()
        };
        let done = spawn(-0.1);
        let waiting = spawn(2.0);

        world
            .run_system_once(respawn_dead_players)
            .expect("the system runs");

        let at = |world: &World, entity| world.get::<Transform>(entity).map(|t| t.translation);
        assert_eq!(at(&world, done), Some(Vec3::new(12000.0, floor, 15000.0)));
        assert!(!world.entity(done).contains::<Dying>());
        assert_eq!(world.get::<BugState>(done), Some(&BugState::Stand));
        assert_eq!(at(&world, waiting), Some(Vec3::new(13000.0, 0.0, 16000.0)));
        assert!(world.entity(waiting).contains::<Dying>());
        let sent: Vec<_> = world
            .resource_mut::<Messages<PlayerRespawned>>()
            .drain()
            .collect();
        assert_eq!(sent, [PlayerRespawned(done)]);
    }
}
