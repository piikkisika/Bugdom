//! The fire wall: a line of flames across a tunnel, which burns the player
//! and goes out a while after its water valve opens.
//!
//! Port of `AddFireWall` and `MoveFireWall` (original/src/Items/Traps.c).
//! The flames hurt through their particles' `HURT_PLAYER` flags, as in the
//! original; the wall itself has no collision.

use bevy::prelude::*;

use super::{emit_burst, valve_is_open};
use crate::effects::{
    EffectsSystems, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture,
};
use crate::items::kind as item;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItemSource, forget_terrain_item,
};
use crate::liquids::WaterValves;
use crate::math::GameRandom;
use crate::player::PlayerSystems;
use crate::state::AppState;
use crate::terrain::{TILE_SIZE, TerrainMap};

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::FIRE_WALL, add_fire_wall)
        .add_systems(
            FixedUpdate,
            (put_out_fire_walls, burn_fire_walls)
                .chain()
                .after(PlayerSystems::Move)
                .before(EffectsSystems::MoveParticles)
                .run_if(in_state(AppState::InGame)),
        );
}

/// A wall's length when its item gives none, in tiles.
const DEFAULT_LENGTH_TILES: u8 = 3;
/// How many flames each tile of wall gets per burst.
const FLAMES_PER_TILE: usize = 3;
/// Seconds between bursts of flames.
const FLAME_INTERVAL: f32 = 0.06;
/// Seconds an open valve takes to put the wall out.
const EXTINGUISH_TIME: f32 = 3.0;
/// How far a flame starts from its point on the wall, in units (the whole
/// width): sideways and up.
const FLAME_SPREAD: f32 = 50.0;
const FLAME_HEIGHT_SPREAD: f32 = 30.0;
/// The flames' random sideways speed, their random vertical speed (both
/// whole widths) and their upward drift, in units per second.
const FLAME_SIDEWAYS_SPEED: f32 = 50.0;
const FLAME_VERTICAL_SPEED: f32 = 80.0;
const FLAME_RISE: f32 = 100.0;
/// A flame's scale is this plus up to 1.
const FLAME_MIN_SCALE: f32 = 2.1;
const FLAME_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(
        ParticleFlags::HURT_PLAYER.0 | ParticleFlags::HURT_PLAYER_BAD.0 | ParticleFlags::HOT.0,
    ),
    // Negative: the flames rise.
    gravity: -100.0,
    magnetism: 9000.0,
    base_scale: 25.0,
    decay_rate: 0.1,
    fade_rate: 0.7,
    texture: ParticleTexture::Fire,
};

/// Which way a wall runs from its item's position (`WallRot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallDirection {
    /// Along +x (`0`, "-").
    X,
    /// Along +x and +z (`1`, "\").
    Diagonal,
    /// Along +z (`2`, "|").
    Z,
    /// Any other number: the original doesn't move along the wall, so all
    /// its flames are at the start.
    None,
}

impl WallDirection {
    fn from_param(param: u8) -> Self {
        match param {
            0 => Self::X,
            1 => Self::Diagonal,
            2 => Self::Z,
            _ => Self::None,
        }
    }

    /// The step from one flame to the next, `step` units long on each
    /// axis it runs along.
    fn step(self, step: f32) -> Vec2 {
        match self {
            Self::X => Vec2::new(step, 0.0),
            Self::Diagonal => Vec2::splat(step),
            Self::Z => Vec2::new(0.0, step),
            Self::None => Vec2::ZERO,
        }
    }
}

/// A burning wall.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FireWall {
    /// The water valve that puts it out (`ValveID`).
    pub valve: u8,
    direction: WallDirection,
    /// Its length in tiles (`WallLength`).
    length: u8,
    /// Seconds until the next burst of flames (`PTimer`).
    flame_timer: f32,
    /// Seconds its valve has been open (`ExtinguishTimer`).
    extinguish_timer: f32,
    flames: Option<ParticleGroupId>,
}

impl FireWall {
    fn new(params: [u8; 4]) -> Self {
        let length = match params[2] {
            0 => DEFAULT_LENGTH_TILES,
            length => length,
        };
        Self {
            valve: params[0],
            direction: WallDirection::from_param(params[1]),
            length,
            flame_timer: 0.0,
            extinguish_timer: 0.0,
            flames: None,
        }
    }

    /// How many flames a burst has.
    fn flame_count(&self) -> usize {
        usize::from(self.length) * FLAMES_PER_TILE
    }

    /// The distance between flames along each axis the wall runs along.
    fn flame_step(&self) -> Vec2 {
        let length = f32::from(self.length) * TILE_SIZE;
        self.direction.step(length / self.flame_count() as f32)
    }

    /// Counts the time the wall's valve has been open; true once it is out.
    fn extinguish(&mut self, valve_open: bool, dt: f32) -> bool {
        if !valve_open {
            return false;
        }
        self.extinguish_timer += dt;
        self.extinguish_timer > EXTINGUISH_TIME
    }

    /// Counts down to the next burst; true when one is due.
    fn flames_due(&mut self, dt: f32) -> bool {
        self.flame_timer -= dt;
        if self.flame_timer < 0.0 {
            self.flame_timer = FLAME_INTERVAL;
            true
        } else {
            false
        }
    }
}

/// Port of `AddFireWall`. `params[0]` is the valve that puts it out,
/// `params[1]` which way it runs and `params[2]` its length in tiles (0 for
/// the default). The original adds it on any level.
fn add_fire_wall(In(spawn): In<ItemSpawn>, mut commands: Commands, map: Res<TerrainMap>) -> bool {
    let (x, z) = (spawn.position.x, spawn.position.y);
    commands.spawn((
        Name::new("Fire wall"),
        Transform::from_xyz(x, map.floor_height(x, z), z),
        FireWall::new(spawn.params),
        TerrainItemSource(spawn.index),
        DespawnOutOfRange,
        DespawnOnExit(AppState::InGame),
    ));
    true
}

/// Puts out the walls whose valve has been open long enough, for good.
/// Port of the first part of `MoveFireWall`.
fn put_out_fire_walls(
    time: Res<Time>,
    mut commands: Commands,
    valves: Res<WaterValves>,
    mut walls: Query<(Entity, &mut FireWall)>,
) {
    for (entity, mut wall) in &mut walls {
        let open = valve_is_open(&valves, wall.valve);
        if wall.extinguish(open, time.delta_secs()) {
            let mut wall = commands.entity(entity);
            forget_terrain_item(&mut wall);
            wall.despawn();
        }
    }
}

/// Spews the walls' flames. Port of the rest of `MoveFireWall`.
fn burn_fire_walls(
    time: Res<Time>,
    map: Res<TerrainMap>,
    valves: Res<WaterValves>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut walls: Query<(&mut FireWall, &Transform)>,
) {
    for (mut wall, transform) in &mut walls {
        // Gone this tick, in `put_out_fire_walls`.
        if wall.extinguish_timer > EXTINGUISH_TIME && valve_is_open(&valves, wall.valve) {
            continue;
        }
        if !wall.flames_due(time.delta_secs()) {
            continue;
        }
        let start = transform.translation.xz();
        let step = wall.flame_step();
        let count = wall.flame_count();
        let wall = &mut *wall;
        emit_burst(
            &mut groups,
            &mut random,
            &mut wall.flames,
            FLAME_GROUP,
            count,
            true,
            |random, i| {
                let along = start + step * i as f32;
                let x = along.x + (random.next_f32() - 0.5) * FLAME_SPREAD;
                let z = along.y + (random.next_f32() - 0.5) * FLAME_SPREAD;
                let y = map.floor_height(x, z) + (random.next_f32() - 0.5) * FLAME_HEIGHT_SPREAD;
                let velocity = Vec3::new(
                    (random.next_f32() - 0.5) * FLAME_SIDEWAYS_SPEED,
                    (random.next_f32() - 0.5) * FLAME_VERTICAL_SPEED + FLAME_RISE,
                    (random.next_f32() - 0.5) * FLAME_SIDEWAYS_SPEED,
                );
                let scale = random.next_f32() + FLAME_MIN_SCALE;
                (Vec3::new(x, y, z), velocity, scale)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_without_a_length_is_three_tiles() {
        let wall = FireWall::new([0, 0, 0, 0]);
        assert_eq!(wall.flame_count(), 9);
        assert_eq!(wall.flame_step(), Vec2::new(3.0 * TILE_SIZE / 9.0, 0.0));
        let diagonal = FireWall::new([0, 1, 2, 0]);
        assert_eq!(diagonal.flame_count(), 6);
        assert_eq!(diagonal.flame_step(), Vec2::splat(2.0 * TILE_SIZE / 6.0));
        assert_eq!(
            FireWall::new([0, 2, 1, 0]).flame_step(),
            Vec2::new(0.0, TILE_SIZE / 3.0)
        );
    }

    #[test]
    fn flames_come_in_bursts() {
        let mut wall = FireWall::new([0; 4]);
        // The first burst is at once, the next after the interval.
        assert!(wall.flames_due(1.0 / 60.0));
        assert!(!wall.flames_due(FLAME_INTERVAL - 0.01));
        assert!(wall.flames_due(0.02));
    }

    #[test]
    fn an_open_valve_puts_the_wall_out_after_three_seconds() {
        let mut wall = FireWall::new([2, 0, 0, 0]);
        assert!(!wall.extinguish(false, 10.0));
        assert!(!wall.extinguish(true, 2.0));
        assert!(!wall.extinguish(false, 2.0));
        assert!(!wall.extinguish(true, 0.9));
        assert!(wall.extinguish(true, 0.2));
    }
}
