//! Static scenery: rocks, plants, trees, posts and wall ends.
//!
//! Port of `AddClover`, `AddGrass`, `AddWeed`, `AddSunFlower`, `AddCosmo`,
//! `AddPoppy`, `AddWallEnd`, `AddTree` and `AddWoodPost`
//! (original/src/Items/Items.c) and `AddRock`, `AddFaucet` and
//! `AddRockLedge` (original/src/Items/Items2.c). The Pond's plants are in
//! `pond.rs` and the Hive's honey tubes in `honey_tube.rs`.

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
        .register_item_kind(item::WALL_END, add_wall_end)
        .register_item_kind(item::TREE, add_tree)
        .register_item_kind(item::WOOD_POST, add_wood_post)
        .register_item_kind(item::FAUCET, add_faucet)
        .register_item_kind(item::ROCK_LEDGE, add_rock_ledge);
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

/// Object types in the Lawn's first model file (`LAWN1_MObjType_*`).
mod lawn1 {
    pub const WATER_FAUCET: usize = 11;
}

/// Object types in the Forest's model file (`FOREST_MObjType_*`).
mod forest {
    pub const TREE: usize = 1;
    pub const GRASS: usize = 2;
    pub const FLAT_ROCK: usize = 10;
    pub const WOOD_POST: usize = 12;
}

/// Object types in the Night's model file (`NIGHT_MObjType_*`).
mod night {
    pub const FLAT_ROCK: usize = 2;
    pub const GRASS: usize = 9;
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
const FOREST_ROCK_SCALE: f32 = 0.9;
const NIGHT_ROCK_SCALE: f32 = 0.6;
/// `TREE_SCALE`
const TREE_SCALE: f32 = 20.0;
const WOOD_POST_SCALE: f32 = 10.0;
/// How far wood posts sink into the ground, in world units.
const WOOD_POST_SINK: f32 = 30.0;
/// Faucets and docks turn in quarter turns (`parm[0] * PI/2`).
pub(super) const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

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
pub(super) fn plant_height(map: &TerrainMap, position: Vec2) -> f32 {
    let (y, normal) = map.height_at(position.x, position.y, LayerKind::Floor);
    if normal.y < SLOPE_NORMAL_Y {
        y - SLOPE_SINK
    } else {
        y
    }
}

/// A square box `height` tall, standing on the origin.
pub(super) fn square_box(height: f32, half_width: f32) -> CollisionBox {
    CollisionBox::new(
        height,
        0.0,
        -half_width,
        half_width,
        half_width,
        -half_width,
    )
}

pub(super) fn misc() -> LayerMask {
    CollisionKind::Misc.into()
}

/// Whether the level is of one of the `allowed` types. The original stops
/// the game with an alert for an item on the wrong level; this skips the
/// item and warns instead.
pub(super) fn on_level(level: &CurrentLevel, allowed: &[LevelType], kind: &str) -> bool {
    let level_type = level.def().level_type;
    let ok = allowed.contains(&level_type);
    if !ok {
        warn!("{kind} items don't belong on {level_type:?} levels");
    }
    ok
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
    if !on_level(&level, &[LevelType::Lawn], "Clover") || variant > 1 {
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

/// The grass model for a level type, or `None` where there is no grass.
/// `variant` is not range checked here.
fn grass_model(level_type: LevelType, variant: usize) -> Option<ModelRef> {
    match level_type {
        LevelType::Lawn => Some(ModelRef::new(ModelFile::Level2, lawn2::GRASS + variant)),
        // The Forest has a single grass; the original would take the next
        // object (a web thread) for variant 1, but no level has one.
        LevelType::Forest => Some(ModelRef::new(ModelFile::Level1, forest::GRASS + variant)),
        LevelType::Night => Some(ModelRef::new(ModelFile::Level1, night::GRASS + variant)),
        _ => None,
    }
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
    let level_type = level.def().level_type;
    let Some(model) = grass_model(level_type, variant) else {
        warn!("Grass items don't belong on {level_type:?} levels");
        return false;
    };
    if variant > 1 {
        return false;
    }
    StaticObject {
        y: plant_height(&map, spawn.position),
        model,
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

/// What a rock looks like and how it collides on each level type.
#[derive(Debug, Clone, PartialEq)]
struct RockShape {
    model: ModelRef,
    scale: f32,
    collision: CollisionBox,
    kinds: LayerMask,
}

/// The rock for `variant` (`params[0]`) on a level type, or `None` where
/// there are no rocks. On the Lawn 0 is the big rock and 1 the little one;
/// on the Forest and the Night 0 is flat and 1 normal.
fn rock_shape(level_type: LevelType, variant: u8) -> Option<RockShape> {
    let mut kinds = LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]);
    let odd = variant & 1 != 0;
    let (model, scale, collision) = match level_type {
        LevelType::Night => (
            ModelRef::new(ModelFile::Level1, night::FLAT_ROCK + usize::from(variant)),
            NIGHT_ROCK_SCALE,
            if odd {
                CollisionBox::new(140.0, -100.0, -100.0, 100.0, 100.0, -100.0)
            } else {
                CollisionBox::new(50.0, -100.0, -100.0, 100.0, 100.0, -100.0)
            },
        ),
        LevelType::Forest => (
            ModelRef::new(ModelFile::Level1, forest::FLAT_ROCK + usize::from(variant)),
            FOREST_ROCK_SCALE,
            if odd {
                CollisionBox::new(210.0, -150.0, -150.0, 150.0, 150.0, -150.0)
            } else {
                CollisionBox::new(75.0, -150.0, -150.0, 150.0, 150.0, -150.0)
            },
        ),
        LevelType::Lawn => (
            ModelRef::new(ModelFile::Level2, lawn2::ROCK + usize::from(variant)),
            LAWN_ROCK_SCALE,
            if odd {
                CollisionBox::new(190.0, -200.0, -150.0, 150.0, 150.0, -150.0)
            } else {
                kinds |= CollisionKind::Impenetrable;
                CollisionBox::new(600.0, -200.0, -360.0, 360.0, 360.0, -360.0)
            },
        ),
        _ => return None,
    };
    Some(RockShape {
        model,
        scale,
        collision,
        kinds,
    })
}

/// Port of `AddRock`.
fn add_rock(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    let level_type = level.def().level_type;
    let Some(rock) = rock_shape(level_type, spawn.params[0]) else {
        warn!("Rock items don't belong on {level_type:?} levels");
        return false;
    };
    // The original turns rocks by the sine of their (whole) x coordinate,
    // so each rock always faces the same way.
    let yaw = (f64::from(spawn.position.x as i32)).sin() as f32 * TAU;
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: rock.model,
        shading: Shading::Lit,
        yaw,
        scale: rock.scale,
        boxes: vec![rock.collision],
        kinds: rock.kinds,
    }
    .spawn("Rock", &mut commands, &mut models, &spawn);
    true
}

/// The tree's seven boxes: the trunk, then an x and a z span for each of
/// its three tiers of branches (`AddTree`).
fn tree_boxes() -> Vec<CollisionBox> {
    let s = TREE_SCALE;
    // (half width along x, half width along z, bottom, top), unscaled.
    let boxes = [
        (25.0, 25.0, 0.0, 616.0),
        (218.0, 5.0, 424.0, 568.0),
        (5.0, 218.0, 424.0, 568.0),
        (256.0, 5.0, 245.0, 409.0),
        (5.0, 256.0, 245.0, 409.0),
        (283.0, 5.0, 48.0, 232.0),
        (5.0, 283.0, 48.0, 232.0),
    ];
    boxes
        .into_iter()
        .map(|(x, z, bottom, top)| {
            CollisionBox::new(top * s, bottom * s, -x * s, x * s, z * s, -z * s)
        })
        .collect()
}

/// Port of `AddTree`. Once added, a tree never goes away.
fn add_tree(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Forest], "Tree") {
        return false;
    }
    let tree = StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level1, forest::TREE),
        shading: Shading::Lit,
        yaw: 0.0,
        scale: TREE_SCALE,
        boxes: tree_boxes(),
        kinds: misc(),
    }
    .spawn("Tree", &mut commands, &mut models, &spawn);
    // The original gives trees no move function, so they are never deleted.
    commands.entity(tree).remove::<DespawnOutOfRange>();
    true
}

/// Port of `AddWoodPost`.
fn add_wood_post(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Forest], "Wood post") {
        return false;
    }
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y) - WOOD_POST_SINK,
        model: ModelRef::new(ModelFile::Level1, forest::WOOD_POST),
        shading: Shading::Lit,
        yaw: 0.0,
        scale: WOOD_POST_SCALE,
        boxes: vec![CollisionBox::new(
            5000.0, -300.0, -550.0, 550.0, 550.0, -550.0,
        )],
        kinds: LayerMask::from([CollisionKind::Misc, CollisionKind::Impenetrable]),
    }
    .spawn("Wood post", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddFaucet`. `params[0]` turns it by quarter turns.
fn add_faucet(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Lawn], "Faucet") {
        return false;
    }
    StaticObject {
        y: map.floor_height(spawn.position.x, spawn.position.y),
        model: ModelRef::new(ModelFile::Level1, lawn1::WATER_FAUCET),
        shading: Shading::Lit,
        yaw: f32::from(spawn.params[0]) * QUARTER_TURN,
        scale: 1.0,
        boxes: vec![square_box(600.0, 110.0)],
        kinds: LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera]),
    }
    .spawn("Faucet", &mut commands, &mut models, &spawn);
    true
}

/// Port of `AddRockLedge`, which adds nothing but still marks its item as
/// added.
fn add_rock_ledge(In(_spawn): In<ItemSpawn>) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rocks_follow_the_level_type() {
        let lawn_big = rock_shape(LevelType::Lawn, 0).unwrap();
        assert!(lawn_big.kinds.has_all(CollisionKind::Impenetrable));
        assert_eq!(lawn_big.collision.top, 600.0);
        let lawn_small = rock_shape(LevelType::Lawn, 1).unwrap();
        assert!(!lawn_small.kinds.has_all(CollisionKind::Impenetrable));

        let forest = rock_shape(LevelType::Forest, 1).unwrap();
        assert_eq!(
            forest.model,
            ModelRef::new(ModelFile::Level1, forest::FLAT_ROCK + 1)
        );
        assert_eq!(forest.scale, FOREST_ROCK_SCALE);
        assert_eq!(forest.collision.top, 210.0);
        assert_eq!(
            rock_shape(LevelType::Forest, 0).unwrap().collision.top,
            75.0
        );

        let night = rock_shape(LevelType::Night, 0).unwrap();
        assert_eq!(
            night.model,
            ModelRef::new(ModelFile::Level1, night::FLAT_ROCK)
        );
        assert_eq!(night.collision.top, 50.0);
        assert_eq!(night.collision.bottom, -100.0);

        assert!(rock_shape(LevelType::Pond, 0).is_none());
    }

    #[test]
    fn grass_comes_from_each_level_types_model_file() {
        assert_eq!(
            grass_model(LevelType::Night, 1),
            Some(ModelRef::new(ModelFile::Level1, night::GRASS + 1))
        );
        assert_eq!(
            grass_model(LevelType::Forest, 0),
            Some(ModelRef::new(ModelFile::Level1, forest::GRASS))
        );
        assert_eq!(grass_model(LevelType::Hive, 0), None);
    }

    #[test]
    fn the_trees_branches_cross_the_trunk() {
        let boxes = tree_boxes();
        assert_eq!(boxes.len(), 7);
        let trunk = boxes[0];
        assert_eq!(trunk.top, 616.0 * TREE_SCALE);
        assert_eq!(trunk.bottom, 0.0);
        for (x_span, z_span) in [(1, 2), (3, 4), (5, 6)] {
            let (x_span, z_span) = (boxes[x_span], boxes[z_span]);
            assert_eq!(x_span.right, z_span.front);
            assert_eq!(x_span.top, z_span.top);
            assert!(x_span.right > trunk.right && x_span.front < trunk.front);
        }
    }
}
