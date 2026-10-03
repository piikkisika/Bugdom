//! Detonators and the hive doors they blow open.
//!
//! Port of `AddDetonator`, `MoveDetonator` and `DoTrig_Detonator`
//! (original/src/Items/Triggers.c), and of `AddHiveDoor`,
//! `MakeOpenHiveDoor` and `MoveHiveDoor` (original/src/Items/Items2.c).
//! Landing on a detonator's plunger pushes it down; once it is all the way
//! down its ID is in [`DetonatorsBlown`], which opens the doors, explodes
//! the firecrackers and bursts the nuts with that ID.

use std::f32::consts::TAU;

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::prelude::*;

use super::model;
use crate::collision::{
    CollisionBox, CollisionKind, CollisionSystems, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::items::kind as item;
use crate::items::pickups::DetonatorsBlown;
use crate::items::scenery::on_level;
use crate::items::triggers::{ChainedTo, ITEM_FLAG_USER1};
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource, TerrainItems,
};
use crate::level::{CurrentLevel, LevelType};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::player::PlayerSystems;
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::DETONATOR, add_detonator)
        .register_item_kind(item::HIVE_DOOR, add_hive_door)
        .add_systems(
            FixedUpdate,
            (
                // The detonator moves early in the object list, before the
                // player (`TRIGGER_SLOT - 1`).
                move_plungers
                    .after(ItemSystems::Track)
                    .before(CollisionSystems::Gather)
                    .before(PlayerSystems::Move),
                (push_plungers, open_hive_doors)
                    .chain()
                    .after(PlayerSystems::Move),
            )
                .run_if(in_state(AppState::InGame)),
        );
}

/// `DETONATOR_SCALE`
const DETONATOR_SCALE: f32 = 1.1;
/// How far below the box's base a plunger that is all the way down sits
/// (`PLUNGER_DOWN_YOFF`).
const PLUNGER_DOWN_OFFSET: f32 = 60.0 * DETONATOR_SCALE;
/// How far below the box's base a plunger that is up sits.
const PLUNGER_UP_OFFSET: f32 = 10.0;
/// How fast a plunger goes down, in units per second.
const PLUNGER_SPEED: f32 = 140.0;
const DETONATOR_BOX: CollisionBox =
    CollisionBox::new(40.0 * DETONATOR_SCALE, 0.0, -30.0, 30.0, 30.0, -30.0);
const PLUNGER_BOX: CollisionBox =
    CollisionBox::new(185.0 * DETONATOR_SCALE, 0.0, -30.0, 30.0, 30.0, -30.0);

/// `HIVE_DOOR_SCALE`
const HIVE_DOOR_SCALE: f32 = 7.0;

/// A detonator's plunger, the part the player lands on.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Plunger {
    /// The detonator's ID (`DetonatorID`).
    pub id: u8,
    /// Its map item, whose flags remember that it was pushed.
    item: u32,
    /// Going down (`IsPlunging`).
    plunging: bool,
    /// Its height when all the way down.
    bottom_y: f32,
}

impl Plunger {
    /// Moves a plunging plunger down for `dt` seconds and returns whether
    /// it has just reached the bottom.
    ///
    /// The plunger part of `MoveDetonator`.
    fn step(&mut self, y: &mut f32, dt: f32) -> bool {
        if !self.plunging {
            return false;
        }
        *y -= PLUNGER_SPEED * dt;
        if *y <= self.bottom_y {
            *y = self.bottom_y;
            self.plunging = false;
            return true;
        }
        false
    }
}

/// A plunger's collision kinds before and after it is pushed.
fn plunger_kinds(pushed: bool) -> CollisionLayers {
    let kinds = if pushed {
        LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera])
    } else {
        LayerMask::from([
            CollisionKind::Trigger,
            CollisionKind::PlayerTriggerOnly,
            CollisionKind::AutoTarget,
            CollisionKind::AutoTargetJump,
            CollisionKind::BlockCamera,
        ])
    };
    CollisionLayers::new(kinds, LayerMask::NONE)
}

/// Port of `AddDetonator`. `params[0]` is the detonator's ID and
/// `params[1]` its colour. The box is solid; the plunger on top is the
/// trigger, and stays down once pushed (`ITEM_FLAGS_USER1`).
fn add_detonator(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive], "Detonator") {
        return false;
    }
    let pushed = spawn.flags & ITEM_FLAG_USER1 != 0;
    let base = Vec3::new(
        spawn.position.x,
        map.floor_height(spawn.position.x, spawn.position.y),
        spawn.position.y,
    );
    let scale = Transform::from_scale(Vec3::splat(DETONATOR_SCALE));

    let detonator = commands
        .spawn((
            Name::new("Detonator"),
            Transform::from_translation(base),
            Visibility::default(),
            solid_object(
                vec![DETONATOR_BOX],
                [CollisionKind::Misc, CollisionKind::BlockCamera],
                SolidSides::ALL,
            ),
            // The original keeps the item on the plunger and tracks the
            // box; the plunger goes with the box (`ChainNode`).
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        &mut commands,
        detonator,
        ModelRef::new(
            ModelFile::Level1,
            model::DETONATOR_GREEN + usize::from(spawn.params[1]),
        ),
        Shading::Lit,
        scale,
    );

    let offset = if pushed {
        PLUNGER_DOWN_OFFSET
    } else {
        PLUNGER_UP_OFFSET
    };
    let position = base - Vec3::Y * offset;
    let plunger = commands
        .spawn((
            Name::new("Plunger"),
            Transform::from_translation(position),
            Visibility::default(),
            Plunger {
                id: spawn.params[0],
                item: spawn.index,
                plunging: false,
                bottom_y: base.y - PLUNGER_DOWN_OFFSET,
            },
            solid_object(vec![PLUNGER_BOX], LayerMask::NONE, SolidSides::ALL),
            plunger_kinds(pushed),
            Trigger {
                sides: SolidSides::TOP,
                solid: true,
            },
            TransformInterpolation,
            ChainedTo(detonator),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(&mut commands, plunger, model::PLUNGER, Shading::Lit, scale);
    true
}

/// Starts pushing down a plunger the player landed on, and remembers it in
/// the map item. Port of `DoTrig_Detonator`.
fn push_plungers(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut plungers: Query<&mut Plunger>,
    mut items: Option<ResMut<TerrainItems>>,
) {
    for hit in hits.read() {
        let Ok(mut plunger) = plungers.get_mut(hit.trigger) else {
            continue;
        };
        // Only `CTYPE_MISC` from now on: no longer a trigger.
        commands
            .entity(hit.trigger)
            .insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
        plunger.plunging = true;
        if let Some(items) = items.as_mut() {
            items.set_flags(plunger.item, ITEM_FLAG_USER1);
        }
        // Sound: EFFECT_PLUNGER at the plunger.
    }
}

/// Moves pushed plungers down; one that reaches the bottom blows its
/// detonator. Port of the plunger part of `MoveDetonator`.
fn move_plungers(
    time: Res<Time>,
    mut blown: ResMut<DetonatorsBlown>,
    mut plungers: Query<(&mut Plunger, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (mut plunger, mut transform) in &mut plungers {
        if !plunger.plunging {
            continue;
        }
        if plunger.step(&mut transform.translation.y, dt) {
            blown.0.insert(plunger.id);
        }
    }
}

/// A closed hive door, which opens when its detonator is blown.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct HiveDoor {
    /// The detonator that opens it (`DetonatorID`).
    pub id: u8,
    /// Which way it faces, in quarter turns (`DoorAim`).
    rot: u8,
    color: u8,
    /// Its closed model, which the open one replaces.
    model: Option<Entity>,
}

/// The closed door's box: across its doorway.
fn closed_door_box(rot: u8) -> CollisionBox {
    let s = HIVE_DOOR_SCALE;
    if rot & 1 != 0 {
        CollisionBox::new(136.0 * s, 0.0, -40.0 * s, 40.0 * s, 146.0 * s, -146.0 * s)
    } else {
        CollisionBox::new(136.0 * s, 0.0, -146.0 * s, 146.0 * s, 40.0 * s, -40.0 * s)
    }
}

/// The open door's boxes: the two sides of the frame, the lintel over the
/// hole and the sill. Port of the boxes of `MakeOpenHiveDoor`, which are
/// in world space there and relative to the door here.
fn open_door_boxes(rot: u8) -> Vec<CollisionBox> {
    let s = HIVE_DOOR_SCALE;
    // (top, bottom, from, to): the span along the door, before scaling.
    let parts: [(f32, f32, f32, f32); 4] = if rot & 1 != 0 {
        [
            (136.0, 0.0, 32.0, 146.0),
            (136.0, 61.0, -32.0, 32.0),
            (136.0, 0.0, -146.0, -32.0),
            (6.0, 0.0, -32.0, 32.0),
        ]
    } else {
        [
            (136.0, 0.0, -146.0, -32.0),
            (136.0, 61.0, -32.0, 32.0),
            (136.0, 0.0, 32.0, 146.0),
            (6.0, 0.0, -32.0, 32.0),
        ]
    };
    parts
        .into_iter()
        .map(|(top, bottom, from, to)| {
            if rot & 1 != 0 {
                CollisionBox::new(top * s, bottom * s, -30.0 * s, 30.0 * s, to * s, from * s)
            } else {
                CollisionBox::new(top * s, bottom * s, from * s, to * s, 30.0 * s, -30.0 * s)
            }
        })
        .collect()
}

fn open_door_model(color: u8) -> ModelRef {
    ModelRef::new(
        ModelFile::Level1,
        model::HIVE_DOOR_GREEN_OPEN + usize::from(color),
    )
}

/// Port of `AddHiveDoor`. `params[0]` is the detonator that opens it,
/// `params[1]` which way it faces, in quarter turns, and `params[2]` its
/// colour. If the detonator is already blown it is made open.
fn add_hive_door(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive], "Hive door") {
        return false;
    }
    let id = spawn.params[0];
    let rot = spawn.params[1];
    let color = spawn.params[2];
    let open = blown.is_blown(id);
    let (boxes, kinds) = if open {
        (open_door_boxes(rot), LayerMask::from(CollisionKind::Misc))
    } else {
        (
            vec![closed_door_box(rot)],
            LayerMask::from([CollisionKind::Misc, CollisionKind::Impenetrable]),
        )
    };
    let door = commands
        .spawn((
            Name::new("Hive door"),
            Transform::from_xyz(
                spawn.position.x,
                map.floor_height(spawn.position.x, spawn.position.y),
                spawn.position.y,
            ),
            Visibility::default(),
            solid_object(boxes, kinds, SolidSides::ALL),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    let model = if open {
        open_door_model(color)
    } else {
        ModelRef::new(
            ModelFile::Level1,
            model::HIVE_DOOR_GREEN + usize::from(color),
        )
    };
    let model = models.spawn(
        &mut commands,
        door,
        model,
        Shading::Lit,
        door_transform(rot),
    );
    if !open {
        commands.entity(door).insert(HiveDoor {
            id,
            rot,
            color,
            model,
        });
    }
    true
}

fn door_transform(rot: u8) -> Transform {
    Transform::from_rotation(Quat::from_rotation_y(f32::from(rot) * (TAU / 4.0)))
        .with_scale(Vec3::splat(HIVE_DOOR_SCALE))
}

/// Swaps a closed door for the open one once its detonator is blown. Port
/// of `MoveHiveDoor`.
///
/// The original deletes the closed door, which frees its map item, and
/// makes an open door that belongs to no item. Here the door is opened in
/// place and keeps its item, so that the item can't be added a second time
/// while the open door is still there; once out of range it comes back
/// open, as in the original.
fn open_hive_doors(
    mut commands: Commands,
    mut models: ModelSpawner,
    blown: Res<DetonatorsBlown>,
    doors: Query<(Entity, &HiveDoor)>,
) {
    for (entity, door) in &doors {
        if !blown.is_blown(door.id) {
            continue;
        }
        if let Some(model) = door.model {
            commands.entity(model).despawn();
        }
        commands
            .entity(entity)
            .remove::<HiveDoor>()
            .insert(solid_object(
                open_door_boxes(door.rot),
                CollisionKind::Misc,
                SolidSides::ALL,
            ));
        models.spawn(
            &mut commands,
            entity,
            open_door_model(door.color),
            Shading::Lit,
            door_transform(door.rot),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plunger_goes_down_until_it_blows() {
        let mut plunger = Plunger {
            id: 3,
            item: 0,
            plunging: false,
            bottom_y: -PLUNGER_DOWN_OFFSET,
        };
        let mut y = -PLUNGER_UP_OFFSET;
        assert!(!plunger.step(&mut y, 0.1));
        assert_eq!(y, -PLUNGER_UP_OFFSET);
        plunger.plunging = true;
        let mut steps = 0;
        while !plunger.step(&mut y, 0.1) {
            steps += 1;
        }
        // 56 units at 140 per second.
        assert!((3..=4).contains(&steps));
        assert_eq!(y, -PLUNGER_DOWN_OFFSET);
        assert!(!plunger.plunging);
    }

    #[test]
    fn open_door_boxes_frame_the_doorway() {
        let s = HIVE_DOOR_SCALE;
        let across_x = open_door_boxes(0);
        assert_eq!(across_x[0].left, -146.0 * s);
        assert_eq!(across_x[0].right, -32.0 * s);
        assert_eq!(across_x[1].bottom, 61.0 * s);
        assert_eq!(across_x[2].left, 32.0 * s);
        assert_eq!(across_x[3].top, 6.0 * s);
        assert_eq!(across_x[3].front, 30.0 * s);
        let across_z = open_door_boxes(1);
        assert_eq!(across_z[0].front, 146.0 * s);
        assert_eq!(across_z[0].back, 32.0 * s);
        assert_eq!(across_z[2].front, -32.0 * s);
        assert_eq!(across_z[2].back, -146.0 * s);
        assert_eq!(across_z[1].left, -30.0 * s);
    }
}
