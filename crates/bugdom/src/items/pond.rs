//! The Pond's plants and docks.
//!
//! Port of `AddCatTail`, `AddDuckWeed`, `AddLilyFlower`, `AddLilyPad`,
//! `AddPondGrass` and `AddReed` (original/src/Items/Items.c) and `AddDock`
//! (original/src/Items/Items2.c).

use std::f32::consts::TAU;

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::kind as item;
use super::scenery::{QUARTER_TURN, StaticObject, misc, on_level, square_box};
use super::{ItemSpawn, RegisterItemKind};
use crate::collision::{CollisionBox, CollisionKind};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::CAT_TAIL, add_cat_tail)
        .register_item_kind(item::DUCK_WEED, add_duck_weed)
        .register_item_kind(item::LILY_FLOWER, add_lily_flower)
        .register_item_kind(item::LILY_PAD, add_lily_pad)
        .register_item_kind(item::POND_GRASS, add_pond_grass)
        .register_item_kind(item::REED, add_reed)
        .register_item_kind(item::DOCK, add_dock);
}

/// Object types in the Pond's model file (`POND_MObjType_*`).
mod model {
    pub const CAT_TAIL: usize = 0;
    pub const DUCK_WEED: usize = 1;
    pub const LILY_FLOWER: usize = 2;
    pub const LILY_PAD: usize = 3;
    pub const POND_GRASS: usize = 4;
    pub const REED: usize = 7;
    pub const DOCK: usize = 10;
}

/// The Pond's water surface, which lilies and docks float on (`WATER_Y`).
const WATER_Y: f32 = 0.0;

/// Cat tails and duck weed are this big plus up to [`SMALL_PLANT_SCALE_RANGE`].
const SMALL_PLANT_SCALE: f32 = 0.15;
const SMALL_PLANT_SCALE_RANGE: f32 = 0.1;
const LILY_FLOWER_SCALE: f32 = 3.0;
const LILY_FLOWER_SCALE_RANGE: f32 = 0.5;
const LILY_PAD_SCALE: f32 = 2.5;
const POND_GRASS_SCALE: f32 = 0.25;
const POND_GRASS_SCALE_RANGE: f32 = 0.1;
const REED_SCALE: f32 = 0.4;
const DOCK_SCALE: f32 = 1.0;

/// The highest `params[0]` of pond grass and of reeds.
const POND_GRASS_VARIANTS: u8 = 3;
const REED_VARIANTS: u8 = 2;

fn on_pond(level: &CurrentLevel, kind: &str) -> bool {
    on_level(level, &[LevelType::Pond], kind)
}

/// A random yaw, then a scale `base` plus up to `range`, drawn in the
/// original's order (`rot` before `scale`).
fn random_yaw_and_scale(random: &mut GameRandom, base: f32, range: f32) -> (f32, f32) {
    let yaw = random.next_f32() * TAU;
    let scale = base + random.next_f32() * range;
    (yaw, scale)
}

/// An unlit plant (`STATUS_BIT_NULLSHADER`) of the Pond's model file.
fn plant(model: usize, y: f32, yaw: f32, scale: f32, boxes: CollisionBox) -> StaticObject {
    plant_with_kinds(model, y, yaw, scale, boxes, misc())
}

fn plant_with_kinds(
    model: usize,
    y: f32,
    yaw: f32,
    scale: f32,
    collision: CollisionBox,
    kinds: LayerMask,
) -> StaticObject {
    StaticObject {
        y,
        model: ModelRef::new(ModelFile::Level1, model),
        shading: Shading::Unlit,
        yaw,
        scale,
        boxes: vec![collision],
        kinds,
    }
}

/// Port of `AddCatTail`.
fn add_cat_tail(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_pond(&level, "Cat tail") {
        return false;
    }
    let (yaw, scale) =
        random_yaw_and_scale(&mut random, SMALL_PLANT_SCALE, SMALL_PLANT_SCALE_RANGE);
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    plant(model::CAT_TAIL, y, yaw, scale, square_box(300.0, 20.0)).spawn(
        "Cat tail",
        &mut commands,
        &mut models,
        &spawn,
    );
    true
}

/// Port of `AddDuckWeed`.
fn add_duck_weed(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_pond(&level, "Duck weed") {
        return false;
    }
    let (yaw, scale) =
        random_yaw_and_scale(&mut random, SMALL_PLANT_SCALE, SMALL_PLANT_SCALE_RANGE);
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    plant(model::DUCK_WEED, y, yaw, scale, square_box(1000.0, 20.0)).spawn(
        "Duck weed",
        &mut commands,
        &mut models,
        &spawn,
    );
    true
}

/// Port of `AddLilyFlower`.
fn add_lily_flower(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_pond(&level, "Lily flower") {
        return false;
    }
    let (yaw, scale) =
        random_yaw_and_scale(&mut random, LILY_FLOWER_SCALE, LILY_FLOWER_SCALE_RANGE);
    plant_with_kinds(
        model::LILY_FLOWER,
        WATER_Y,
        yaw,
        scale,
        CollisionBox::new(100.0, -100.0, -150.0, 150.0, 150.0, -150.0),
        LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]),
    )
    .spawn("Lily flower", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddLilyPad`: a pad the player can stand on but not go through.
fn add_lily_pad(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_pond(&level, "Lily pad") {
        return false;
    }
    let yaw = random.next_f32() * TAU;
    plant_with_kinds(
        model::LILY_PAD,
        WATER_Y,
        yaw,
        LILY_PAD_SCALE,
        CollisionBox::new(15.0, -900.0, -400.0, 400.0, 400.0, -400.0),
        LayerMask::from([
            CollisionKind::Misc,
            CollisionKind::BlockShadow,
            CollisionKind::BlockCamera,
            CollisionKind::Impenetrable,
            CollisionKind::Impenetrable2,
        ]),
    )
    .spawn("Lily pad", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddPondGrass`. `params[0]` picks one of three grasses.
fn add_pond_grass(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let variant = spawn.params[0];
    if !on_pond(&level, "Pond grass") || variant >= POND_GRASS_VARIANTS {
        return false;
    }
    let (yaw, scale) = random_yaw_and_scale(&mut random, POND_GRASS_SCALE, POND_GRASS_SCALE_RANGE);
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    plant(
        model::POND_GRASS + usize::from(variant),
        y,
        yaw,
        scale,
        square_box(1000.0, 20.0),
    )
    .spawn("Pond grass", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddReed`. `params[0]` picks a thin (0) or a thick (1) reed.
fn add_reed(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut random: ResMut<GameRandom>,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let variant = spawn.params[0];
    if !on_pond(&level, "Reed") || variant >= REED_VARIANTS {
        return false;
    }
    let yaw = random.next_f32() * TAU;
    let half_width = if variant == 0 { 35.0 } else { 90.0 };
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    plant(
        model::REED + usize::from(variant),
        y,
        yaw,
        REED_SCALE,
        square_box(1000.0, half_width),
    )
    .spawn("Reed", &mut commands, &mut models, &spawn);
    true
}

/// The dock's box for its quarter turns, longer along z unless turned a
/// quarter or three quarters of the way round.
fn dock_box(turns: u8) -> CollisionBox {
    if turns & 1 != 0 {
        CollisionBox::new(35.0, -200.0, -215.0, 215.0, 127.0, -127.0)
    } else {
        CollisionBox::new(35.0, -200.0, -127.0, 127.0, 215.0, -215.0)
    }
}

/// Port of `AddDock`. `params[0]` turns it by quarter turns.
fn add_dock(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_pond(&level, "Dock") {
        return false;
    }
    let turns = spawn.params[0];
    StaticObject {
        y: WATER_Y,
        model: ModelRef::new(ModelFile::Level1, model::DOCK),
        shading: Shading::Lit,
        yaw: f32::from(turns) * QUARTER_TURN,
        scale: DOCK_SCALE,
        boxes: vec![dock_box(turns)],
        kinds: LayerMask::from([
            CollisionKind::Misc,
            CollisionKind::BlockShadow,
            CollisionKind::BlockCamera,
        ]),
    }
    .spawn("Dock", &mut commands, &mut models, &spawn);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_dock_turns_its_box() {
        let straight = dock_box(0);
        let turned = dock_box(1);
        assert_eq!(straight.front, turned.right);
        assert_eq!(straight.right, turned.front);
        assert_eq!(dock_box(2), straight);
        assert_eq!(dock_box(3), turned);
    }

    #[test]
    fn yaw_is_drawn_before_scale() {
        let mut a = GameRandom::default();
        let mut b = a.clone();
        let (yaw, scale) = random_yaw_and_scale(&mut a, 1.0, 0.5);
        assert_eq!(yaw, b.next_f32() * TAU);
        assert_eq!(scale, 1.0 + b.next_f32() * 0.5);
    }
}
