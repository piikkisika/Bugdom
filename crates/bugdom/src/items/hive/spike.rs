//! Floor spikes: spikes hidden in the Hive's floor that shoot up when a
//! player comes near.
//!
//! Port of `AddFloorSpike` and `MoveFloorSpike`
//! (original/src/Items/Traps.c).

use avian3d::prelude::{LayerMask, TransformInterpolation};
use bevy::prelude::*;

use super::model;
use crate::collision::{CollisionBox, CollisionKind, CollisionSystems, SolidSides, solid_object};
use crate::combat::Damage;
use crate::items::kind as item;
use crate::items::scenery::on_level;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::quick_distance;
use crate::objects::{ModelSpawner, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{Player, PlayerSystems};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::FLOOR_SPIKE, add_floor_spike)
        .add_systems(
            FixedUpdate,
            move_floor_spikes
                .after(ItemSystems::Track)
                .before(CollisionSystems::Gather)
                .before(PlayerSystems::Move)
                .run_if(in_state(AppState::InGame)),
        );
}

/// How far the spike reaches above the floor, in units (`SPIKE_HEIGHT`).
const SPIKE_HEIGHT: f32 = 200.0;
/// How close a player must come to set it off, in units (`SPIKE_DIST`).
const SPIKE_DIST: f32 = 150.0;
/// How deep the hidden spike sits under the floor, in units.
const SPIKE_SINK: f32 = 5.0;
/// How fast it shoots up and goes back down, in units per second.
const SPIKE_SPEED: f32 = 500.0;
const SPIKE_DAMAGE: f32 = 0.2;
/// Its box hangs below its tip.
const SPIKE_BOX: CollisionBox = CollisionBox::new(0.0, -SPIKE_HEIGHT, -20.0, 20.0, 20.0, -20.0);

/// A floor spike and what it is doing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FloorSpike {
    /// Hidden, waiting for a player (`SPIKE_MODE_WAIT`).
    #[default]
    Wait,
    GoUp,
    GoDown,
}

impl FloorSpike {
    /// Moves the spike for `dt` seconds. `velocity` is its vertical
    /// velocity, which the original keeps while waiting (its `Delta.y`
    /// stays at the downward speed). `near` is whether a player is in
    /// reach.
    ///
    /// The `Mode` switch of `MoveFloorSpike`.
    fn step(&mut self, y: &mut f32, velocity: &mut f32, floor: f32, near: bool, dt: f32) {
        match self {
            Self::Wait => {
                if near {
                    *self = Self::GoUp;
                    *velocity = SPIKE_SPEED;
                }
            }
            Self::GoUp => {
                *y += *velocity * dt;
                let top = floor + SPIKE_HEIGHT;
                if *y > top {
                    *y = top;
                    *self = Self::GoDown;
                    *velocity = -SPIKE_SPEED;
                }
            }
            Self::GoDown => {
                *y += *velocity * dt;
                let bottom = floor - SPIKE_SINK;
                if *y < bottom {
                    *y = bottom;
                    *self = Self::Wait;
                }
            }
        }
    }
}

/// Port of `AddFloorSpike`.
fn add_floor_spike(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive], "Floor spike") {
        return false;
    }
    let position = Vec3::new(
        spawn.position.x,
        map.floor_height(spawn.position.x, spawn.position.y) - SPIKE_SINK,
        spawn.position.y,
    );
    let spike = commands
        .spawn((
            Name::new("Floor spike"),
            Transform::from_translation(position),
            Visibility::default(),
            FloorSpike::Wait,
            Damage(SPIKE_DAMAGE),
            solid_object(
                vec![SPIKE_BOX],
                LayerMask::from([CollisionKind::Misc, CollisionKind::HurtMe]),
                SolidSides::ALL,
            ),
            Velocity::default(),
            PreviousPosition(position),
            TransformInterpolation,
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        &mut commands,
        spike,
        model::FLOOR_SPIKE,
        Shading::Lit,
        Transform::default(),
    );
    true
}

/// Shoots the spikes up at nearby players and pulls them back down. Port
/// of `MoveFloorSpike`; leaving the item window is [`DespawnOutOfRange`].
///
/// The original moves spikes after the player, and the player sees the
/// spike's move the next frame through its old collision box. Here spikes
/// move before the player's collision, which gives the same view of the
/// move; the nearness test uses where the players were at the end of the
/// previous tick. Any player in reach sets a spike off.
fn move_floor_spikes(
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut spikes: Query<(&mut FloorSpike, &mut Transform, &mut Velocity), Without<Player>>,
    players: Query<&Transform, With<Player>>,
) {
    let dt = time.delta_secs();
    for (mut spike, mut transform, mut velocity) in &mut spikes {
        let position = transform.translation;
        let near = players
            .iter()
            .any(|player| quick_distance(position.xz(), player.translation.xz()) <= SPIKE_DIST);
        let floor = map.floor_height(position.x, position.z);
        let mut y = position.y;
        spike.step(&mut y, &mut velocity.y, floor, near, dt);
        if y != position.y {
            transform.translation.y = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spike_waits_for_a_player_then_shoots_up_and_back_down() {
        let floor = 100.0;
        let mut spike = FloorSpike::Wait;
        let mut y = floor - SPIKE_SINK;
        let mut v = 0.0;
        spike.step(&mut y, &mut v, floor, false, 0.1);
        assert_eq!(spike, FloorSpike::Wait);
        spike.step(&mut y, &mut v, floor, true, 0.1);
        assert_eq!(spike, FloorSpike::GoUp);
        assert_eq!(y, floor - SPIKE_SINK);
        for _ in 0..5 {
            spike.step(&mut y, &mut v, floor, false, 0.1);
        }
        assert_eq!(spike, FloorSpike::GoDown);
        assert_eq!(y, floor + SPIKE_HEIGHT);
        assert_eq!(v, -SPIKE_SPEED);
        for _ in 0..5 {
            spike.step(&mut y, &mut v, floor, false, 0.1);
        }
        assert_eq!(spike, FloorSpike::Wait);
        assert_eq!(y, floor - SPIKE_SINK);
        // Its downward speed stays while it waits.
        assert_eq!(v, -SPIKE_SPEED);
    }
}
