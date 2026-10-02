//! Opening the lawn doors with their keys.
//!
//! Port of `MoveLawnDoor` and `DoTrig_LawnDoor`
//! (original/src/Items/Triggers.c). The door itself is spawned by
//! `AddLawnDoor` in `items/triggers.rs`.

use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use crate::collision::TriggerHit;
use crate::items::triggers::{ITEM_FLAG_USER1, KeyDoor};
use crate::items::{TerrainItemSource, TerrainItems};
use crate::level::{CurrentLevel, LevelType};
use crate::objects::ObjectModel;
use crate::player::{DoorKey, Inventory, Player, PlayerForm, PlayerSystems};
use crate::state::AppState;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        (open_doors, swing_doors)
            .chain()
            .after(PlayerSystems::Move)
            .run_if(in_state(AppState::InGame)),
    );
}

/// How fast a door swings open, in radians per second.
const DOOR_OPEN_SPEED: f32 = 1.5;

/// A door swinging open (`DOOR_MODE_OPENING`). It is removed once the door
/// is open.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct DoorSwing {
    /// The door's turn about y (`Rot.y`).
    yaw: f32,
    /// In radians per second; the sign is the way it turns.
    speed: f32,
    /// Where it stops (`DoorSwingMax`).
    end: f32,
}

impl DoorSwing {
    /// Which way a door facing `aim` opens away from a player at
    /// `player`, in x and z. Port of `DoTrig_LawnDoor`'s switch on
    /// `DoorAim`.
    fn away_from(aim: u8, door: Vec2, player: Vec2) -> Self {
        let yaw = f32::from(aim) * FRAC_PI_2;
        let backward = match aim & 3 {
            // Hinged on the left: a player behind it opens it forward.
            0 => player.y < door.y,
            // Hinged at the front: a player on its left opens it right.
            1 => player.x < door.x,
            // Hinged on the right.
            2 => player.y >= door.y,
            // Hinged at the back.
            _ => player.x >= door.x,
        };
        let speed = if backward {
            -DOOR_OPEN_SPEED
        } else {
            DOOR_OPEN_SPEED
        };
        Self {
            yaw,
            speed,
            end: yaw + speed.signum() * FRAC_PI_2,
        }
    }

    /// Turns the door for `dt` seconds; returns whether it is now open.
    /// Port of `MoveLawnDoor`'s `DOOR_MODE_OPENING`.
    fn step(&mut self, dt: f32) -> bool {
        self.yaw += self.speed * dt;
        let open = if self.speed < 0.0 {
            self.yaw <= self.end
        } else {
            self.yaw >= self.end
        };
        if open {
            self.yaw = self.end;
        }
        open
    }
}

/// Opens the doors that a player on foot holding their key walks into,
/// using up the key. Port of `DoTrig_LawnDoor`.
fn open_doors(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    doors: Query<(&KeyDoor, &Transform, Option<&TerrainItemSource>), Without<DoorSwing>>,
    mut players: Query<(&PlayerForm, &Transform, &mut Inventory), (With<Player>, Without<KeyDoor>)>,
    items: Option<ResMut<TerrainItems>>,
    level: Res<CurrentLevel>,
) {
    let mut items = items;
    let mut opened = EntityHashSet::default();
    for hit in hits.read() {
        let Ok((door, transform, source)) = doors.get(hit.trigger) else {
            continue;
        };
        let Ok((form, player, mut inventory)) = players.get_mut(hit.mover) else {
            continue;
        };
        // Doors don't open for the ball.
        if *form == PlayerForm::Ball {
            continue;
        }
        let Some(key) = DoorKey::from_number(door.key) else {
            continue;
        };
        if !inventory.has_key(key) || !opened.insert(hit.trigger) {
            continue;
        }
        let swing = DoorSwing::away_from(
            door.aim,
            transform.translation.xz(),
            player.translation.xz(),
        );
        // No collision while it opens, nor once it is open.
        commands.entity(hit.trigger).insert((
            swing,
            CollisionLayers::new(LayerMask::NONE, LayerMask::NONE),
        ));
        // It stays open when it comes back.
        if let (Some(TerrainItemSource(index)), Some(items)) = (source, items.as_mut()) {
            items.set_flags(*index, ITEM_FLAG_USER1);
        }
        inventory.use_key(key);
        match level.def().level_type {
            LevelType::Lawn => {
                // Sound: EFFECT_OPENLAWNDOOR at the door.
            }
            LevelType::Night => {
                // Sound: EFFECT_OPENNIGHTDOOR at the door.
            }
            _ => {}
        }
    }
}

/// Swings the opening doors. Port of `MoveLawnDoor`; going out of range
/// is the door's [`DespawnOutOfRange`](crate::items::DespawnOutOfRange).
fn swing_doors(
    time: Res<Time>,
    mut commands: Commands,
    mut doors: Query<(Entity, &mut DoorSwing, &Children)>,
    mut models: Query<&mut Transform, With<ObjectModel>>,
) {
    for (entity, mut swing, children) in &mut doors {
        if swing.step(time.delta_secs()) {
            commands.entity(entity).remove::<DoorSwing>();
        }
        for child in children {
            if let Ok(mut model) = models.get_mut(*child) {
                model.rotation = Quat::from_rotation_y(swing.yaw);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    #[test]
    fn a_door_opens_away_from_the_player() {
        let door = Vec2::new(1000.0, 1000.0);
        let behind = Vec2::new(1000.0, 900.0);
        let in_front = Vec2::new(1000.0, 1100.0);
        let left = Vec2::new(900.0, 1000.0);
        let right = Vec2::new(1100.0, 1000.0);

        let swing = DoorSwing::away_from(0, door, behind);
        assert_eq!((swing.speed, swing.end), (-DOOR_OPEN_SPEED, -FRAC_PI_2));
        let swing = DoorSwing::away_from(0, door, in_front);
        assert_eq!((swing.speed, swing.end), (DOOR_OPEN_SPEED, FRAC_PI_2));
        let swing = DoorSwing::away_from(1, door, left);
        assert_eq!((swing.speed, swing.end), (-DOOR_OPEN_SPEED, 0.0));
        let swing = DoorSwing::away_from(2, door, behind);
        assert_eq!((swing.speed, swing.end), (DOOR_OPEN_SPEED, PI + FRAC_PI_2));
        let swing = DoorSwing::away_from(3, door, right);
        assert_eq!(swing.speed, -DOOR_OPEN_SPEED);
        assert!((swing.end - PI).abs() < 1e-6);
    }

    #[test]
    fn the_key_opens_its_door_for_good() {
        use bevy::ecs::system::RunSystemOnce;
        use bugdom_formats::terrain::Item;

        let mut world = World::new();
        world.insert_resource(CurrentLevel(1));
        world.init_resource::<Messages<TriggerHit>>();
        let item = Item {
            x: 0,
            z: 0,
            kind: crate::items::kind::LAWN_DOOR,
            params: [1, 0, 0, 0],
            flags: 0,
        };
        world.insert_resource(TerrainItems::new(vec![item]));
        let door = world
            .spawn((
                KeyDoor { key: 1, aim: 0 },
                Transform::default(),
                TerrainItemSource(0),
            ))
            .id();
        let mut inventory = Inventory::default();
        inventory.get_key(DoorKey::Blue);
        let ball = world
            .spawn((
                Player,
                PlayerForm::Ball,
                Transform::from_xyz(0.0, 0.0, 100.0),
                inventory.clone(),
            ))
            .id();
        let bug = world
            .spawn((
                Player,
                PlayerForm::Bug,
                Transform::from_xyz(0.0, 0.0, 100.0),
                inventory,
            ))
            .id();
        let hit = |mover| TriggerHit {
            trigger: door,
            mover,
            sides: crate::collision::SolidSides::BACK,
        };
        world.write_message(hit(ball));
        world.run_system_once(open_doors).expect("the system runs");
        // Not for the ball.
        assert!(!world.entity(door).contains::<DoorSwing>());

        world.write_message(hit(bug));
        world.run_system_once(open_doors).expect("the system runs");
        let swing = world.get::<DoorSwing>(door).copied();
        assert_eq!(swing.map(|s| s.speed), Some(DOOR_OPEN_SPEED));
        let layers = world.get::<CollisionLayers>(door).copied();
        assert_eq!(layers.map(|l| l.memberships), Some(LayerMask::NONE));
        let has_key = |world: &World, player| {
            world
                .get::<Inventory>(player)
                .is_some_and(|i| i.has_key(DoorKey::Blue))
        };
        assert!(!has_key(&world, bug));
        assert!(has_key(&world, ball));
        assert_eq!(
            world.resource::<TerrainItems>().items[0].flags,
            ITEM_FLAG_USER1
        );
    }

    #[test]
    fn a_door_stops_once_open() {
        let mut swing = DoorSwing::away_from(0, Vec2::ZERO, Vec2::new(0.0, 10.0));
        assert!(!swing.step(0.5));
        assert_eq!(swing.yaw, 0.75);
        assert!(swing.step(1.0));
        assert_eq!(swing.yaw, FRAC_PI_2);
    }
}
