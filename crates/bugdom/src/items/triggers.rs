//! Items the player sets off: checkpoints, doors and the exit log.
//!
//! Port of `AddCheckpoint`, `MoveCheckpoint`, `DoTrig_Checkpoint`,
//! `AddExitLog` and `DoTrig_ExitLog` (original/src/Items/Triggers2.c), and
//! `AddLawnDoor` (original/src/Items/Triggers.c).

use std::f32::consts::{FRAC_PI_2, TAU};

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::scenery::{StaticObject, item};
use super::{ItemSpawn, ItemSystems, RegisterItemKind};
use crate::collision::{
    CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectModel, Shading};
use crate::player::PlayerSystems;
use crate::state::AppState;
use crate::terrain::{PlayerStart, TerrainMap, TerrainSystems};

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::CHECKPOINT, add_checkpoint)
        .register_item_kind(item::LAWN_DOOR, add_lawn_door)
        .register_item_kind(item::EXIT_LOG, add_exit_log)
        .add_systems(
            OnEnter(AppState::InGame),
            reset_level_progress
                .after(TerrainSystems::Spawn)
                .before(ItemSystems::Window),
        )
        .add_systems(
            FixedUpdate,
            (
                (tag_checkpoints, complete_area).after(PlayerSystems::Move),
                wobble_droplets,
            )
                .run_if(in_state(AppState::InGame)),
        );
}

/// `GLOBAL1_MObjType_Straw` and `GLOBAL1_MObjType_Droplet`.
const STRAW_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 7);
const DROPLET_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 8);
/// `GLOBAL2_MObjType_ExitLog`
const EXIT_LOG_MODEL: ModelRef = ModelRef::new(ModelFile::Global2, 1);
/// `LAWN1_MObjType_Door_Green`; the other colours follow.
const LAWN_DOOR_MODEL: usize = 1;

const CHECKPOINT_SCALE: f32 = 1.5;
/// Where the droplet hangs from the straw, before scaling.
const DROPLET_OFFSET: Vec2 = Vec2::new(192.0, 224.0);
const DROPLET_OPACITY: f32 = 0.6;
/// How fast the droplet's x, y and z wobble, in radians per second.
const DROPLET_WOBBLE_RATE: Vec3 = Vec3::new(8.0, 9.0, 7.0);
/// How much the droplet's scale wobbles.
const DROPLET_WOBBLE: f32 = 0.4;
const LOG_SCALE: f32 = 6.0;
const LAWN_DOOR_SCALE: f32 = 0.6;
/// The item flag set once a door has been opened (`ITEM_FLAGS_USER1`).
pub const ITEM_FLAG_USER1: u16 = 1 << 1;

/// The checkpoint the player restarts from (`gBestCheckPoint`,
/// `gMostRecentCheckPointCoord` and `gCheckPointRot`).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct Checkpoints {
    /// The highest checkpoint number reached, if any.
    pub best: Option<u8>,
    pub position: Vec3,
    pub yaw: f32,
}

/// Set when the player reaches the level's exit (`gAreaCompleted`).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq, Deref, DerefMut)]
pub struct AreaCompleted(pub bool);

/// A checkpoint's droplet, which the player touches to tag it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct CheckpointDroplet {
    pub number: u8,
    /// The direction the player faces when restarting here.
    pub player_yaw: f32,
    /// Wobble angles for the scale in x, y and z.
    wobble: Vec3,
}

/// The trigger at the end of the exit log.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExitTrigger;

/// A door opened with a key (`TRIGTYPE_TWIGDOOR`). Opening it arrives with
/// keys.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDoor {
    pub key: u8,
    /// Which way it faces, in quarter turns (`DoorAim`).
    pub aim: u8,
}

/// Parts of an object that go when it goes (`ChainNode`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = ChainedParts)]
pub struct ChainedTo(pub Entity);

#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
#[relationship_target(relationship = ChainedTo, linked_spawn)]
pub struct ChainedParts(Vec<Entity>);

/// Port of the checkpoint and exit set-up in `InitPlayerAtStartOfLevel`
/// (original/src/Player/MyGuy.c) and `InitArea` (original/src/System/Main.c).
fn reset_level_progress(mut commands: Commands, start: Res<PlayerStart>, map: Res<TerrainMap>) {
    let (x, z) = (start.position.x, start.position.y);
    commands.insert_resource(Checkpoints {
        best: None,
        position: Vec3::new(x, map.floor_height(x, z), z),
        // Rounded down to a quarter turn.
        yaw: f32::from(start.aim / 2) * (TAU / 4.0),
    });
    commands.insert_resource(AreaCompleted(false));
}

/// Port of `AddCheckpoint`. `params[0]` is the checkpoint's number and
/// `params[1]` the player's direction there, in quarter turns. Only
/// checkpoints after the best one reached get a droplet.
fn add_checkpoint(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    checkpoints: Res<Checkpoints>,
) -> bool {
    let number = spawn.params[0];
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    let straw = StaticObject {
        y,
        model: STRAW_MODEL,
        shading: Shading::Lit,
        yaw: 0.0,
        scale: CHECKPOINT_SCALE,
        boxes: vec![CollisionBox::new(300.0, 0.0, -20.0, 20.0, 20.0, -20.0)],
        kinds: LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]),
    }
    .spawn("Checkpoint", &mut commands, &mut models, &spawn);

    if checkpoints.best.is_some_and(|best| number <= best) {
        return true;
    }
    let offset = DROPLET_OFFSET * CHECKPOINT_SCALE;
    let s = CHECKPOINT_SCALE;
    let wobble = Vec3::new(random.next_f32(), random.next_f32(), random.next_f32());
    let droplet = commands
        .spawn((
            Name::new("Checkpoint droplet"),
            Transform::from_xyz(spawn.position.x + offset.x, y + offset.y, spawn.position.y),
            Visibility::default(),
            CheckpointDroplet {
                number,
                player_yaw: f32::from(spawn.params[1]) * (TAU / 4.0),
                wobble,
            },
            Trigger {
                sides: SolidSides::ALL,
                solid: false,
            },
            solid_object(
                vec![CollisionBox::new(
                    0.0,
                    -73.0 * s,
                    -21.0 * s,
                    21.0 * s,
                    21.0 * s,
                    -21.0 * s,
                )],
                [
                    CollisionKind::Trigger,
                    CollisionKind::PlayerTriggerOnly,
                    CollisionKind::AutoTarget,
                    CollisionKind::AutoTargetJump,
                ],
                SolidSides::ALL,
            ),
            ChainedTo(straw),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn_with_opacity(
        &mut commands,
        droplet,
        DROPLET_MODEL,
        Shading::Lit,
        DROPLET_OPACITY,
        Transform::from_scale(Vec3::splat(CHECKPOINT_SCALE)),
    );
    true
}

/// Wobbles the droplets' scale. Port of `MoveCheckpoint`.
fn wobble_droplets(
    time: Res<Time>,
    mut droplets: Query<(&mut CheckpointDroplet, &Children)>,
    mut models: Query<&mut Transform, With<ObjectModel>>,
) {
    for (mut droplet, children) in &mut droplets {
        droplet.wobble += DROPLET_WOBBLE_RATE * time.delta_secs();
        let scale = Vec3::splat(CHECKPOINT_SCALE) + droplet.wobble.map(f32::sin) * DROPLET_WOBBLE;
        for child in children {
            if let Ok(mut transform) = models.get_mut(*child) {
                transform.scale = scale;
            }
        }
    }
}

/// Records the checkpoint and pops the droplet. Port of
/// `DoTrig_Checkpoint`; the sparks and the sound arrive with particles and
/// audio.
fn tag_checkpoints(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    droplets: Query<(&CheckpointDroplet, &Transform)>,
    mut checkpoints: ResMut<Checkpoints>,
) {
    for hit in hits.read() {
        let Ok((droplet, transform)) = droplets.get(hit.trigger) else {
            continue;
        };
        if checkpoints.best.is_none_or(|best| droplet.number > best) {
            *checkpoints = Checkpoints {
                best: Some(droplet.number),
                position: transform.translation,
                yaw: droplet.player_yaw,
            };
        }
        commands.entity(hit.trigger).despawn();
    }
}

/// Port of `DoTrig_ExitLog`.
fn complete_area(
    mut hits: MessageReader<TriggerHit>,
    exits: Query<(), With<ExitTrigger>>,
    mut completed: ResMut<AreaCompleted>,
) {
    for hit in hits.read() {
        if exits.contains(hit.trigger) && !**completed {
            info!("Area completed");
            **completed = true;
        }
    }
}

/// Port of `AddLawnDoor` for the Lawn. `params[0]` is the key that opens it
/// and `params[1]` which way it faces, in quarter turns.
fn add_lawn_door(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if level.def().level_type != LevelType::Lawn {
        warn!(
            "Doors on {:?} levels are not ported yet",
            level.def().level_type
        );
        return false;
    }
    let key = spawn.params[0];
    let mut aim = spawn.params[1];
    if spawn.flags & ITEM_FLAG_USER1 != 0 {
        // Already opened: swung round a quarter turn.
        aim ^= 1;
    }
    // Hinged at the door's origin, reaching 600 units along its aim.
    let door_box = match aim & 3 {
        0 => CollisionBox::new(700.0, 0.0, 0.0, 600.0, 40.0, -40.0),
        1 => CollisionBox::new(700.0, 0.0, -40.0, 40.0, 0.0, -600.0),
        2 => CollisionBox::new(700.0, 0.0, -600.0, 0.0, 40.0, -40.0),
        _ => CollisionBox::new(700.0, 0.0, -40.0, 40.0, 600.0, 0.0),
    };
    let door = StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level1, LAWN_DOOR_MODEL + usize::from(key)),
        shading: Shading::Lit,
        yaw: f32::from(aim) * FRAC_PI_2,
        scale: LAWN_DOOR_SCALE,
        boxes: vec![door_box],
        kinds: LayerMask::from([
            CollisionKind::Trigger,
            CollisionKind::PlayerTriggerOnly,
            CollisionKind::BlockCamera,
            CollisionKind::Impenetrable,
            CollisionKind::Misc,
        ]),
    }
    .spawn("Door", &mut commands, &mut models, &spawn);
    commands.entity(door).insert((
        KeyDoor { key, aim },
        Trigger {
            sides: SolidSides::ALL,
            solid: true,
        },
    ));
    true
}

/// Port of `AddExitLog`. `params[0]` is which way it faces, in quarter
/// turns. The log is solid along its sides and over its top; a trigger at
/// its far end completes the area.
fn add_exit_log(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
) -> bool {
    let rot = spawn.params[0];
    let s = LOG_SCALE;
    let walls = |left: f32, right: f32, front: f32, back: f32, bottom: f32| {
        CollisionBox::new(
            90.0 * s,
            bottom * s,
            left * s,
            right * s,
            front * s,
            back * s,
        )
    };
    // The two side walls and the roof between them. Facing 2, the original
    // swaps every box's front and back, so that log has no collision; kept.
    let boxes = match rot {
        0 => vec![
            walls(-63.0, -30.0, 104.0, -68.0, 0.0),
            walls(30.0, 63.0, 104.0, -68.0, 0.0),
            walls(-30.0, 30.0, 104.0, -68.0, 56.0),
        ],
        1 => vec![
            walls(-68.0, 104.0, -30.0, -63.0, 0.0),
            walls(-68.0, 104.0, 63.0, 30.0, 0.0),
            walls(-68.0, 104.0, 30.0, -30.0, 56.0),
        ],
        2 => vec![
            walls(-63.0, -30.0, -68.0, 104.0, 0.0),
            walls(30.0, 63.0, -68.0, 104.0, 0.0),
            walls(-30.0, 30.0, -68.0, 104.0, 56.0),
        ],
        3 => vec![
            walls(-104.0, 68.0, -30.0, -63.0, 0.0),
            walls(-104.0, 68.0, 63.0, 30.0, 0.0),
            walls(-104.0, 68.0, 30.0, -30.0, 56.0),
        ],
        _ => {
            warn!("Exit log with direction {rot}");
            return false;
        }
    };
    let end_box = match rot {
        0 => CollisionBox::new(56.0 * s, 0.0, -30.0 * s, 30.0 * s, 80.0 * s, 24.0 * s),
        1 => CollisionBox::new(56.0 * s, 0.0, 24.0 * s, 80.0 * s, 30.0 * s, -30.0 * s),
        2 => CollisionBox::new(56.0 * s, 0.0, -30.0 * s, 30.0 * s, -24.0 * s, -80.0 * s),
        _ => CollisionBox::new(56.0 * s, 0.0, -80.0 * s, -24.0 * s, 30.0 * s, -30.0 * s),
    };

    let y = map.floor_height(spawn.position.x, spawn.position.y);
    let log = StaticObject {
        y,
        model: EXIT_LOG_MODEL,
        shading: Shading::Lit,
        yaw: f32::from(rot) * FRAC_PI_2,
        scale: LOG_SCALE,
        boxes,
        kinds: LayerMask::from([
            CollisionKind::Misc,
            CollisionKind::BlockCamera,
            CollisionKind::Impenetrable,
        ]),
    }
    .spawn("Exit log", &mut commands, &mut models, &spawn);

    commands.spawn((
        Name::new("Exit trigger"),
        Transform::from_xyz(spawn.position.x, y, spawn.position.y),
        ExitTrigger,
        Trigger {
            sides: SolidSides::ALL,
            solid: false,
        },
        solid_object(
            vec![end_box],
            [CollisionKind::Trigger, CollisionKind::PlayerTriggerOnly],
            SolidSides::ALL,
        ),
        ChainedTo(log),
        DespawnOnExit(AppState::InGame),
    ));
    true
}
