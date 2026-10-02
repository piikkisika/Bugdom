//! Static scenery: rocks, plants and wall ends.
//!
//! Port of `AddClover`, `AddGrass`, `AddWeed`, `AddSunFlower`, `AddCosmo`,
//! `AddPoppy` and `AddWallEnd` (original/src/Items/Items.c) and `AddRock`
//! (original/src/Items/Items2.c), for the Lawn. The other level types'
//! variants arrive with those levels.

use std::f32::consts::TAU;

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::kind as item;
use super::{DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItemSource};
use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::state::AppState;
use crate::terrain::{LayerKind, TerrainMap};

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::ROCK, add_rock)
        .register_item_kind(item::CLOVER, add_clover)
        .register_item_kind(item::GRASS, add_grass)
        .register_item_kind(item::WEED, add_weed)
        .register_item_kind(item::SUNFLOWER, add_sunflower)
        .register_item_kind(item::COSMO, add_cosmo)
        .register_item_kind(item::POPPY, add_poppy)
        .register_item_kind(item::WALL_END, add_wall_end);
}

/// Object types in the Lawn's second model file (`LAWN2_MObjType_*`).
mod lawn2 {
    pub const GRASS: usize = 0;
    pub const WEED: usize = 2;
    pub const COSMO: usize = 3;
    pub const POPPY: usize = 4;
    pub const SUNFLOWER: usize = 5;
    pub const CLOVER: usize = 6;
    pub const ROCK: usize = 8;
}

/// `GLOBAL1_MObjType_WallEnd`
const WALL_END_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 4);

/// Plants on a slope steeper than this sink in a little.
const SLOPE_NORMAL_Y: f32 = 0.85;
/// How far plants on a slope sink, in world units.
const SLOPE_SINK: f32 = 40.0;

const CLOVER_SCALE: f32 = 0.15;
const GRASS_SCALE: f32 = 0.15;
const WEED_SCALE: f32 = 0.2;
const COSMO_SCALE: f32 = 0.4;
const POPPY_SCALE: f32 = 0.4;
const SUNFLOWER_SCALE: f32 = 0.15;
const LAWN_ROCK_SCALE: f32 = 4.0;

/// An object that stays where it is put and is deleted when out of range
/// (`MoveStaticObject`).
pub(super) struct StaticObject {
    pub y: f32,
    pub model: ModelRef,
    pub shading: Shading,
    pub yaw: f32,
    pub scale: f32,
    pub boxes: Vec<CollisionBox>,
    pub kinds: LayerMask,
}

impl StaticObject {
    /// Spawns the object for `spawn` and returns its root entity.
    pub fn spawn(
        self,
        name: &'static str,
        commands: &mut Commands,
        models: &mut ModelSpawner,
        spawn: &ItemSpawn,
    ) -> Entity {
        let root = commands
            .spawn((
                Name::new(name),
                Transform::from_xyz(spawn.position.x, self.y, spawn.position.y),
                Visibility::default(),
                TerrainItemSource(spawn.index),
                DespawnOutOfRange,
                DespawnOnExit(AppState::InGame),
                solid_object(self.boxes, self.kinds, SolidSides::ALL),
            ))
            .id();
        models.spawn(
            commands,
            root,
            self.model,
            self.shading,
            Transform::from_rotation(Quat::from_rotation_y(self.yaw))
                .with_scale(Vec3::splat(self.scale)),
        );
        root
    }
}

/// The floor height for a plant, sunk a little on slopes.
fn plant_height(map: &TerrainMap, position: Vec2) -> f32 {
    let (y, normal) = map.height_at(position.x, position.y, LayerKind::Floor);
    if normal.y < SLOPE_NORMAL_Y {
        y - SLOPE_SINK
    } else {
        y
    }
}

/// A square box `height` tall, standing on the origin.
fn square_box(height: f32, half_width: f32) -> CollisionBox {
    CollisionBox::new(
        height,
        0.0,
        -half_width,
        half_width,
        half_width,
        -half_width,
    )
}

fn misc() -> LayerMask {
    CollisionKind::Misc.into()
}

fn on_lawn(level: &CurrentLevel, kind: &str) -> bool {
    let lawn = level.def().level_type == LevelType::Lawn;
    if !lawn {
        warn!(
            "{kind} items on {:?} levels are not ported yet",
            level.def().level_type
        );
    }
    lawn
}

/// Port of `AddClover`. `params[0]` picks one of two clovers.
fn add_clover(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let variant = usize::from(spawn.params[0]);
    if !on_lawn(&level, "Clover") || variant > 1 {
        return false;
    }
    let yaw = random.next_f32() * TAU;
    let s = CLOVER_SCALE + random.next_f32() * 0.1;
    StaticObject {
        y: plant_height(&map, spawn.position),
        model: ModelRef::new(ModelFile::Level2, lawn2::CLOVER + variant),
        shading: Shading::Unlit,
        yaw,
        scale: s,
        boxes: vec![square_box(2100.0 * s, 200.0 * s)],
        kinds: misc(),
    }
    .spawn("Clover", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddGrass`. `params[0]` picks one of two grasses.
fn add_grass(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let variant = usize::from(spawn.params[0]);
    if !on_lawn(&level, "Grass") || variant > 1 {
        return false;
    }
    StaticObject {
        y: plant_height(&map, spawn.position),
        model: ModelRef::new(ModelFile::Level2, lawn2::GRASS + variant),
        shading: Shading::Unlit,
        yaw: random.next_f32() * TAU,
        scale: GRASS_SCALE,
        boxes: vec![square_box(6500.0 * GRASS_SCALE, 500.0 * GRASS_SCALE)],
        kinds: LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]),
    }
    .spawn("Grass", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddWeed`.
fn add_weed(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
) -> bool {
    StaticObject {
        y: plant_height(&map, spawn.position),
        model: ModelRef::new(
            ModelFile::Level2,
            lawn2::WEED + usize::from(spawn.params[0]),
        ),
        shading: Shading::Unlit,
        yaw: random.next_f32() * TAU,
        scale: WEED_SCALE,
        boxes: vec![square_box(5000.0 * WEED_SCALE, 570.0 * WEED_SCALE)],
        kinds: misc(),
    }
    .spawn("Weed", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddSunFlower`.
fn add_sunflower(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
) -> bool {
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level2, lawn2::SUNFLOWER),
        shading: Shading::Unlit,
        yaw: random.next_f32() * TAU,
        scale: SUNFLOWER_SCALE,
        boxes: vec![square_box(600.0, 40.0)],
        kinds: misc(),
    }
    .spawn("Sunflower", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddCosmo`. The original gives cosmos a little damage but never
/// makes them hurt.
fn add_cosmo(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
) -> bool {
    let yaw = random.next_f32() * TAU;
    let scale = COSMO_SCALE + random.next_f32() * 0.05;
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level2, lawn2::COSMO),
        shading: Shading::Unlit,
        yaw,
        scale,
        // The box uses the base scale, not the random one.
        boxes: vec![square_box(700.0 * COSMO_SCALE, 160.0 * COSMO_SCALE)],
        kinds: misc(),
    }
    .spawn("Cosmo", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddPoppy`.
fn add_poppy(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
) -> bool {
    let yaw = random.next_f32() * TAU;
    let scale = POPPY_SCALE + random.next_f32() * 0.05;
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level2, lawn2::POPPY),
        shading: Shading::Unlit,
        yaw,
        scale,
        boxes: vec![square_box(1900.0 * POPPY_SCALE, 300.0 * POPPY_SCALE)],
        kinds: misc(),
    }
    .spawn("Poppy", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddWallEnd`: the post at the end of a wall.
fn add_wall_end(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let (scale, wall_box) = if level.def().level_type == LevelType::Pond {
        (1.5, square_box(1100.0, 350.0))
    } else {
        (0.7, square_box(500.0, 170.0))
    };
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y) - 30.0,
        model: WALL_END_MODEL,
        shading: Shading::Lit,
        yaw: 0.0,
        scale,
        boxes: vec![wall_box],
        kinds: LayerMask::from([
            CollisionKind::Misc,
            CollisionKind::BlockCamera,
            CollisionKind::Impenetrable,
        ]),
    }
    .spawn("Wall end", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddRock` for the Lawn. `params[0]` is 0 for the big rock and 1
/// for the little one.
fn add_rock(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_lawn(&level, "Rock") {
        return false;
    }
    let variant = spawn.params[0];
    let mut kinds = LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]);
    let rock_box = if variant & 1 != 0 {
        CollisionBox::new(190.0, -200.0, -150.0, 150.0, 150.0, -150.0)
    } else {
        kinds |= CollisionKind::Impenetrable;
        CollisionBox::new(600.0, -200.0, -360.0, 360.0, 360.0, -360.0)
    };
    // The original turns rocks by the sine of their (whole) x coordinate,
    // so each rock always faces the same way.
    let yaw = (f64::from(spawn.position.x as i32)).sin() as f32 * TAU;
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level2, lawn2::ROCK + usize::from(variant)),
        shading: Shading::Lit,
        yaw,
        scale: LAWN_ROCK_SCALE,
        boxes: vec![rock_box],
        kinds,
    }
    .spawn("Rock", &mut commands, &mut models, &spawn);
    true
}
