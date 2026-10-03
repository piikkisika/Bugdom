//! The water valve, which the player turns to flood the underground water
//! and put out the fire walls.
//!
//! Port of `AddWaterValve`, `MoveWaterValve` and `DoTrig_WaterValve`
//! (original/src/Items/Triggers.c).

use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::anthill_models;
use crate::collision::{
    CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::items::kind as item;
use crate::items::scenery::on_level;
use crate::items::triggers::ITEM_FLAG_USER1;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItemSource, TerrainItems,
};
use crate::level::{CurrentLevel, LevelType};
use crate::liquids::WaterValves;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::player::{Player, PlayerSystems};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::WATER_VALVE, add_water_valve)
        .add_systems(
            FixedUpdate,
            open_valves
                .after(PlayerSystems::Move)
                .run_if(in_state(AppState::InGame)),
        );
}

const VALVE_BOX_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, anthill_models::WATER_VALVE_BOX);
const VALVE_HANDLE_MODEL: ModelRef =
    ModelRef::new(ModelFile::Level1, anthill_models::WATER_VALVE_HANDLE);
const VALVE_SCALE: f32 = 0.25;
/// How far above the box the handle sits, in units.
const HANDLE_HEIGHT: f32 = 100.0;
/// How far the handle turns when the valve opens, in radians.
const HANDLE_TURN: f32 = FRAC_PI_2;
const VALVE_BOX: CollisionBox = CollisionBox::new(100.0, 0.0, -30.0, 30.0, 30.0, -30.0);

/// A water valve, shut until the player touches it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterValve {
    /// Which entry of [`WaterValves`] it opens (`ValveID`).
    pub id: u8,
    /// The handle's model, which turns when it opens (`ChainNode`).
    handle: Option<Entity>,
    open: bool,
}

/// The collision kinds of a shut valve, which the player sets off and
/// jumps at.
fn shut_kinds() -> LayerMask {
    LayerMask::from([
        CollisionKind::Trigger,
        CollisionKind::PlayerTriggerOnly,
        CollisionKind::AutoTarget,
        CollisionKind::AutoTargetJump,
        CollisionKind::BlockCamera,
    ])
}

/// Port of `AddWaterValve`. `params[0]` is the valve's id. A valve the
/// player has opened before comes back open, but with its handle as it was
/// (as in the original).
fn add_water_valve(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::AntHill], "Water valve") {
        return false;
    }
    let open = spawn.flags & ITEM_FLAG_USER1 != 0;
    let kinds = if open {
        LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera])
    } else {
        shut_kinds()
    };
    let (x, z) = (spawn.position.x, spawn.position.y);
    let root = commands
        .spawn((
            Name::new("Water valve"),
            Transform::from_xyz(x, map.floor_height(x, z), z),
            Visibility::default(),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
            solid_object(vec![VALVE_BOX], kinds, SolidSides::ALL),
        ))
        .id();
    if !open {
        commands.entity(root).insert(Trigger {
            sides: SolidSides::ALL,
            solid: true,
        });
    }
    let scale = Transform::from_scale(Vec3::splat(VALVE_SCALE));
    models.spawn(&mut commands, root, VALVE_BOX_MODEL, Shading::Lit, scale);
    let handle = models.spawn(
        &mut commands,
        root,
        VALVE_HANDLE_MODEL,
        Shading::Lit,
        scale.with_translation(Vec3::Y * HANDLE_HEIGHT),
    );
    commands.entity(root).insert(WaterValve {
        id: spawn.params[0],
        handle,
        open,
    });
    true
}

/// Opens the valves the player touches: the valve's water starts to flow,
/// its handle turns, and it stops being a trigger. Port of
/// `DoTrig_WaterValve`.
fn open_valves(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut valves: Query<(&mut WaterValve, &TerrainItemSource)>,
    players: Query<(), With<Player>>,
    mut handles: Query<&mut Transform>,
    mut open: ResMut<WaterValves>,
    mut items: ResMut<TerrainItems>,
) {
    for hit in hits.read() {
        let Ok((mut valve, source)) = valves.get_mut(hit.trigger) else {
            continue;
        };
        if valve.open || !players.contains(hit.mover) {
            continue;
        }
        valve.open = true;
        commands
            .entity(hit.trigger)
            .remove::<Trigger>()
            .insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
        // The item comes back open if it streams out and in again.
        items.set_flags(source.0, ITEM_FLAG_USER1);
        match open.0.get_mut(usize::from(valve.id)) {
            Some(slot) => *slot = true,
            None => warn!("Water valve {} has no water to let out", valve.id),
        }
        if let Some(mut handle) = valve.handle.and_then(|h| handles.get_mut(h).ok()) {
            handle.rotate_y(HANDLE_TURN);
        }
        // Sound: EFFECT_VALVEOPEN at the valve.
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bugdom_formats::terrain::Item;

    use super::*;

    #[test]
    fn touching_a_valve_opens_it_once_and_for_good() {
        let mut world = World::new();
        world.init_resource::<Messages<TriggerHit>>();
        world.init_resource::<WaterValves>();
        let item = Item {
            x: 0,
            z: 0,
            kind: item::WATER_VALVE,
            params: [3, 0, 0, 0],
            flags: 0,
        };
        world.insert_resource(TerrainItems::new(vec![item]));
        let player = world.spawn(Player).id();
        let handle = world.spawn(Transform::default()).id();
        let valve = world
            .spawn((
                WaterValve {
                    id: 3,
                    handle: Some(handle),
                    open: false,
                },
                TerrainItemSource(0),
                Trigger {
                    sides: SolidSides::ALL,
                    solid: true,
                },
            ))
            .id();
        for _ in 0..2 {
            world.write_message(TriggerHit {
                trigger: valve,
                mover: player,
                sides: SolidSides::ALL,
            });
            world.run_system_once(open_valves).unwrap();
        }
        assert!(world.resource::<WaterValves>().0[3]);
        assert!(!world.resource::<WaterValves>().0[2]);
        assert!(!world.entity(valve).contains::<Trigger>());
        assert_eq!(
            world.resource::<TerrainItems>().items[0].flags & ITEM_FLAG_USER1,
            ITEM_FLAG_USER1
        );
        // Turned a quarter, only once.
        let turned = world.get::<Transform>(handle).unwrap().rotation;
        assert!(turned.abs_diff_eq(Quat::from_rotation_y(HANDLE_TURN), 1e-5));
    }
}
