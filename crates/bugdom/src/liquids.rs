//! Liquid patches: water, honey, slime and lava. Each is a flat, textured
//! surface with a collision volume under it; whatever sinks into the volume
//! is [`Underwater`].
//!
//! Port of original/src/Items/Liquids.c.

use avian3d::prelude::LayerMask;
use bevy::math::Affine2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::items::kind as item;
use crate::items::{DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItemSource};
use crate::level::{CurrentLevel, LevelType};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{TILE_SIZE, TerrainMap};
use bevy::asset::RenderAssetUsages;

pub struct LiquidsPlugin;

impl Plugin for LiquidsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LiquidMaterials>()
            .init_resource::<WaterValves>()
            .register_item_kind(item::WATER_PATCH, add_water_patch)
            .register_item_kind(
                item::HONEY_PATCH,
                add_liquid_patch::<{ LiquidKind::Honey as u8 }>,
            )
            .register_item_kind(
                item::SLIME_PATCH,
                add_liquid_patch::<{ LiquidKind::Slime as u8 }>,
            )
            .register_item_kind(
                item::LAVA_PATCH,
                add_liquid_patch::<{ LiquidKind::Lava as u8 }>,
            )
            .add_systems(OnEnter(AppState::InGame), reset_liquids)
            .add_systems(OnExit(AppState::InGame), clear_liquid_materials)
            .add_systems(FixedUpdate, raise_water.run_if(in_state(AppState::InGame)))
            .add_systems(
                Update,
                scroll_liquid_textures.run_if(in_state(AppState::InGame)),
            );
    }
}

/// The kinds of liquid (`LIQUID_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LiquidKind {
    Water,
    Honey,
    Slime,
    Lava,
}

impl LiquidKind {
    const ALL: [Self; 4] = [Self::Water, Self::Honey, Self::Slime, Self::Lava];

    fn from_u8(value: u8) -> Self {
        Self::ALL[usize::from(value).min(3)]
    }

    /// How far below the surface the collision volume starts, so that
    /// shallow liquid doesn't count (`gLiquidCollisionTopOffset`).
    pub fn collision_top_offset(self) -> f32 {
        match self {
            Self::Water => 75.0,
            Self::Honey => 70.0,
            Self::Slime | Self::Lava => 60.0,
        }
    }

    /// The liquids a level type has (`gLiquidOnThisLevel`).
    pub fn on_level(level: LevelType) -> &'static [Self] {
        match level {
            LevelType::Lawn | LevelType::Pond | LevelType::Forest => &[Self::Water],
            LevelType::Hive => &[Self::Honey],
            LevelType::Night => &[Self::Slime],
            LevelType::AntHill => &[Self::Water, Self::Slime, Self::Lava],
        }
    }

    /// The texture file (`InitLiquids`).
    pub fn texture_path(self, level: LevelType) -> String {
        let number = match self {
            Self::Water if level == LevelType::Pond => 129,
            Self::Water => 128,
            Self::Honey => 200,
            Self::Slime => 201,
            Self::Lava => 202,
        };
        format!("Images/Textures/{number}.tga")
    }

    /// Fixed surface heights that items can pick by index
    /// (`gLiquidYTable`).
    fn y_table(self) -> [f32; 6] {
        match self {
            Self::Water => [900.0, 950.0, 0.0, 0.0, 0.0, 0.0],
            Self::Honey => [-620.0, -580.0, -550.0, -600.0, 0.0, 0.0],
            Self::Slime => [0.0, -200.0, 0.0, 0.0, 0.0, 0.0],
            Self::Lava => [-230.0, -230.0, -230.0, 0.0, 0.0, 0.0],
        }
    }

    /// How far the inner vertices of a patch are pushed about, in x and z
    /// and in y (`ApplyJitterToLiquidVertex`).
    fn jitter(self) -> (f32, f32) {
        match self {
            Self::Water => (60.0, 0.0),
            Self::Honey => (40.0, 10.0),
            Self::Slime => (40.0, 0.0),
            Self::Lava => (30.0, 0.0),
        }
    }

    /// How fast the texture scrolls, in texture widths per second
    /// (`UpdateWaterTextureAnimation` and the others).
    fn scroll_rate(self) -> Vec2 {
        match self {
            Self::Water => Vec2::new(0.05, 0.1),
            Self::Honey | Self::Slime | Self::Lava => Vec2::new(0.09, 0.1),
        }
    }
}

/// A liquid patch. Its first collision box is its volume.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liquid(pub LiquidKind);

impl Liquid {
    /// The height of the visible surface above the top of the volume.
    pub fn surface_above_volume(self) -> f32 {
        self.0.collision_top_offset()
    }
}

/// An entity in a liquid's volume (`STATUS_BIT_UNDERWATER`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Underwater {
    /// The top of the liquid's volume, which is below its visible surface
    /// (`gPlayerCurrentWaterY`).
    pub volume_top: f32,
    /// `gCurrentLiquidType`
    pub liquid: LiquidKind,
}

/// Which of the Ant Hill's water valves are open (`gValveIsOpen`). An open
/// valve floods its underground water patches. The valves themselves arrive
/// with the Ant Hill's triggers.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct WaterValves(pub [bool; 8]);

/// Underground water that rises when its valve opens.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct RisingWater {
    valve: usize,
    /// The height it rises to.
    full_y: f32,
}

/// Fixed surface heights of underground water, by valve (`yTable2` in
/// `AddWaterPatch`).
const UNDERGROUND_WATER_Y: [f32; 8] = [
    -540.0, -540.0, -540.0, -540.0, -370.0, -540.0, -540.0, -540.0,
];
/// How far underground water rises when its valve opens
/// (`RISING_WATER_YOFF`).
const RISING_WATER_RISE: f32 = 200.0;
/// How fast it rises, in units per second.
const RISING_WATER_SPEED: f32 = 30.0;
/// The Pond's water is all at this height (`WATER_Y`).
const POND_WATER_Y: f32 = 0.0;
/// The largest patch, in tiles (`MAX_LIQUID_SIZE`).
const MAX_PATCH_TILES: u8 = 20;
/// A patch's size when its item gives none, in tiles.
const DEFAULT_PATCH_TILES: u8 = 4;
/// How deep a patch's volume reaches below its top.
const VOLUME_DEPTH: f32 = 2000.0;

/// The materials of the level's liquids, one per kind, opacity and
/// texture layer, so their textures can scroll together.
#[derive(Resource, Debug, Default)]
struct LiquidMaterials {
    materials: HashMap<(LiquidKind, u32, Layer), Handle<StandardMaterial>>,
    /// How far each kind's texture has scrolled, and the second layer of
    /// untesselated water (`gLiquidUVOffsets`, `gWaterUVOffset2`).
    scroll: [Vec2; 4],
    second_scroll: Vec2,
}

/// The two texture layers of untesselated water; everything else has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Layer {
    First,
    Second,
}

/// How fast the second layer of untesselated water scrolls.
const SECOND_LAYER_SCROLL_RATE: Vec2 = Vec2::new(-0.12, -0.07);

impl LiquidMaterials {
    fn get(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        texture: Handle<Image>,
        kind: LiquidKind,
        opacity: f32,
        layer: Layer,
    ) -> Handle<StandardMaterial> {
        self.materials
            .entry((kind, opacity.to_bits(), layer))
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::WHITE.with_alpha(opacity),
                    base_color_texture: Some(texture),
                    alpha_mode: if opacity < 1.0 {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    // `STATUS_BIT_NULLSHADER`
                    unlit: true,
                    ..default()
                })
            })
            .clone()
    }
}

fn reset_liquids(mut commands: Commands) {
    commands.insert_resource(WaterValves::default());
}

fn clear_liquid_materials(mut liquid_materials: ResMut<LiquidMaterials>) {
    *liquid_materials = LiquidMaterials::default();
}

/// Scrolls the liquids' textures. Port of `UpdateLiquidAnimation`.
fn scroll_liquid_textures(
    time: Res<Time>,
    mut liquid_materials: ResMut<LiquidMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dt = time.delta_secs();
    let liquid_materials = &mut *liquid_materials;
    for kind in LiquidKind::ALL {
        // Wrapped, so that precision doesn't run out on a long level.
        liquid_materials.scroll[kind as usize] =
            (liquid_materials.scroll[kind as usize] + kind.scroll_rate() * dt).fract_gl();
    }
    liquid_materials.second_scroll =
        (liquid_materials.second_scroll + SECOND_LAYER_SCROLL_RATE * dt).fract_gl();
    for (&(kind, _, layer), handle) in &liquid_materials.materials {
        let offset = match layer {
            Layer::First => liquid_materials.scroll[kind as usize],
            Layer::Second => liquid_materials.second_scroll,
        };
        if let Some(mut material) = materials.get_mut(handle) {
            material.uv_transform = Affine2::from_translation(offset);
        }
    }
}

/// Raises underground water whose valve has opened. Port of the Ant Hill
/// part of `MoveLiquidPatch`.
fn raise_water(
    time: Res<Time>,
    valves: Res<WaterValves>,
    mut commands: Commands,
    mut patches: Query<(Entity, &RisingWater, &mut Transform)>,
) {
    for (entity, rising, mut transform) in &mut patches {
        if !valves.0.get(rising.valve).copied().unwrap_or(false) {
            continue;
        }
        transform.translation.y += RISING_WATER_SPEED * time.delta_secs();
        if transform.translation.y >= rising.full_y {
            transform.translation.y = rising.full_y;
            commands.entity(entity).remove::<RisingWater>();
        }
    }
}

/// The resources the patch spawners share.
#[derive(bevy::ecs::system::SystemParam)]
struct PatchSpawner<'w, 's> {
    commands: Commands<'w, 's>,
    level: Res<'w, CurrentLevel>,
    level_assets: Res<'w, LevelAssets>,
    map: Res<'w, TerrainMap>,
    valves: Res<'w, WaterValves>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    liquid_materials: ResMut<'w, LiquidMaterials>,
}

/// What a patch looks like and where it is.
struct Patch {
    kind: LiquidKind,
    center: Vec3,
    /// Size in tiles.
    width: u8,
    depth: u8,
    opacity: f32,
    tesselate: bool,
}

impl PatchSpawner<'_, '_> {
    /// The level's texture for `kind`, if the level has that liquid.
    fn texture(&self, kind: LiquidKind) -> Option<Handle<Image>> {
        self.level_assets
            .liquid_textures
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, texture)| texture.clone())
    }

    /// Spawns a patch and returns its entity, or `None` if the level has no
    /// texture for its liquid.
    fn spawn(&mut self, spawn: &ItemSpawn, patch: Patch) -> Option<Entity> {
        let Some(texture) = self.texture(patch.kind) else {
            // The original stops with `GAME_ASSERT_MESSAGE` here.
            warn!("A {:?} patch is on a level without that liquid", patch.kind);
            return None;
        };
        let half = Vec2::new(f32::from(patch.width), f32::from(patch.depth)) * (TILE_SIZE / 2.0);
        let mut kinds = LayerMask::from([CollisionKind::Liquid, CollisionKind::BlockCamera]);
        if patch.kind != LiquidKind::Water {
            kinds |= CollisionKind::BlockShadow;
        }
        let root = self
            .commands
            .spawn((
                Name::new(format!("{:?} patch", patch.kind)),
                Liquid(patch.kind),
                Transform::from_translation(patch.center),
                Visibility::default(),
                TerrainItemSource(spawn.index),
                DespawnOutOfRange,
                DespawnOnExit(AppState::InGame),
                // The volume starts a little under the surface, so that only
                // sinking deep counts as being in it.
                solid_object(
                    vec![CollisionBox::new(
                        -patch.kind.collision_top_offset(),
                        -VOLUME_DEPTH,
                        -half.x,
                        half.x,
                        half.y,
                        -half.y,
                    )],
                    kinds,
                    SolidSides::TOUCHABLE,
                ),
            ))
            .id();

        let layers: Vec<(Mesh, Layer)> = if patch.tesselate {
            let step = if patch.kind == LiquidKind::Water {
                2
            } else {
                1
            };
            vec![(grid_mesh(&patch, step), Layer::First)]
        } else {
            vec![
                (quad_mesh(&patch, 0.5), Layer::First),
                (quad_mesh(&patch, 1.0 / 3.0), Layer::Second),
            ]
        };
        for (mesh, layer) in layers {
            let material = self.liquid_materials.get(
                &mut self.materials,
                texture.clone(),
                patch.kind,
                patch.opacity,
                layer,
            );
            self.commands.spawn((
                Mesh3d(self.meshes.add(mesh)),
                MeshMaterial3d(material),
                Transform::default(),
                ChildOf(root),
            ));
        }
        Some(root)
    }
}

/// An item's position rounded down to its tile, then to the tile's centre.
fn tile_center(position: Vec2) -> Vec2 {
    let tile = TILE_SIZE as i32;
    let snap = |v: f32| {
        let v = v as i32;
        (v - v % tile) as f32 + TILE_SIZE / 2.0
    };
    Vec2::new(snap(position.x), snap(position.y))
}

/// A patch's size in tiles from its item; zero means the default.
fn patch_size(param: u8) -> Option<u8> {
    let size = if param == 0 {
        DEFAULT_PATCH_TILES
    } else {
        param
    };
    (size <= MAX_PATCH_TILES).then_some(size)
}

/// Port of `AddWaterPatch`. `params[0]` and `params[1]` are the width and
/// depth in tiles; `params[2]` is the height above the floor in units of 4,
/// or an index into a height table, or the valve of underground water;
/// `params[3]` bit 0 asks for a tesselated surface, bit 1 puts the water
/// underground and bit 2 picks the height table.
fn add_water_patch(In(spawn): In<ItemSpawn>, mut spawner: PatchSpawner) -> bool {
    let xz = tile_center(spawn.position);
    let level_type = spawner.level.def().level_type;
    let [width, depth, height, flags] = spawn.params;
    let mut tesselate = flags & 1 != 0 || level_type == LevelType::AntHill;
    let underground = flags & (1 << 1) != 0;

    let mut rising = None;
    let y = if level_type == LevelType::Pond {
        tesselate = true;
        POND_WATER_Y
    } else if underground {
        let valve = usize::from(height);
        let Some(&low_y) = UNDERGROUND_WATER_Y.get(valve) else {
            warn!("Underground water names valve {valve}, which doesn't exist");
            return false;
        };
        let full_y = low_y + RISING_WATER_RISE;
        if spawner.valves.0[valve] {
            full_y
        } else {
            rising = Some(RisingWater { valve, full_y });
            low_y
        }
    } else if flags & (1 << 2) != 0 {
        let Some(&y) = LiquidKind::Water.y_table().get(usize::from(height)) else {
            warn!("Water patch picks height {height}, which isn't in the table");
            return false;
        };
        y
    } else {
        let offset = if height == 0 {
            3.0
        } else {
            f32::from(height) * 4.0
        };
        spawner.map.floor_height(xz.x, xz.y) + offset
    };

    let (Some(width), Some(depth)) = (patch_size(width), patch_size(depth)) else {
        warn!("Water patch {width}×{depth} is larger than {MAX_PATCH_TILES} tiles");
        return false;
    };
    if tesselate && (width % 2 != 0 || depth % 2 != 0) {
        // The original stops with a fatal error; the plain surface still
        // works.
        warn!("Tesselated water must be an even number of tiles, not {width}×{depth}");
        tesselate = false;
    }
    let opacity = match (level_type, tesselate) {
        (LevelType::Pond, true) => 0.6,
        (LevelType::Pond, false) => 0.7,
        _ => 0.5,
    };
    let patch = Patch {
        kind: LiquidKind::Water,
        center: Vec3::new(xz.x, y, xz.y),
        width,
        depth,
        opacity,
        tesselate,
    };
    let Some(root) = spawner.spawn(&spawn, patch) else {
        return false;
    };
    if let Some(rising) = rising {
        spawner.commands.entity(root).insert(rising);
    }
    true
}

/// Port of `AddHoneyPatch`, `AddSlimePatch`, `AddLavaPatch` and
/// `AddLiquidPatch`. `params[0]` and `params[1]` are the width and depth in
/// tiles; `params[2]` is the height above the floor in units of 10, or,
/// with bit 0 of `params[3]`, an index into the liquid's height table.
fn add_liquid_patch<const KIND: u8>(In(spawn): In<ItemSpawn>, mut spawner: PatchSpawner) -> bool {
    let kind = LiquidKind::from_u8(KIND);
    let xz = tile_center(spawn.position);
    let [width, depth, height, flags] = spawn.params;
    let y = if flags & 1 != 0 {
        let Some(&y) = kind.y_table().get(usize::from(height)) else {
            warn!("{kind:?} patch picks height {height}, which isn't in the table");
            return false;
        };
        y
    } else {
        let offset = if height == 0 {
            40.0
        } else {
            f32::from(height) * 10.0
        };
        spawner.map.floor_height(xz.x, xz.y) + offset
    };
    let (Some(width), Some(depth)) = (patch_size(width), patch_size(depth)) else {
        warn!("{kind:?} patch {width}×{depth} is larger than {MAX_PATCH_TILES} tiles");
        return false;
    };
    let patch = Patch {
        kind,
        center: Vec3::new(xz.x, y, xz.y),
        width,
        depth,
        opacity: 1.0,
        tesselate: true,
    };
    spawner.spawn(&spawn, patch).is_some()
}

/// A flat grid over the patch with a vertex every `step` tiles, its inner
/// vertices jittered so that it looks less regular. Port of
/// `DrawWaterPatchTesselated` and `DrawSolidLiquidPatchTesselated`. The
/// texture repeats every 2.5 vertices.
fn grid_mesh(patch: &Patch, step: u8) -> Mesh {
    let columns = u32::from(patch.width / step);
    let rows = u32::from(patch.depth / step);
    let spacing = f32::from(step) * TILE_SIZE;
    let size = Vec2::new(f32::from(patch.width), f32::from(patch.depth)) * TILE_SIZE;
    let back_left = patch.center.xz() - size / 2.0;
    let (jitter_xz, jitter_y) = patch.kind.jitter();

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    for row in 0..=rows {
        for column in 0..=columns {
            // The original jitters world coordinates, so do the same.
            let mut p = Vec3::new(
                back_left.x + column as f32 * spacing,
                patch.center.y,
                back_left.y + row as f32 * spacing,
            );
            if row > 0 && row < rows && column > 0 && column < columns {
                let (x, z) = (p.x, p.z);
                p.x = x + (x * z).sin() * jitter_xz;
                p.y += (z * x).sin() * jitter_y;
                p.z = z + (z * -x).cos() * jitter_xz;
            }
            positions.push((p - patch.center).to_array());
            uvs.push([column as f32 * 0.4, 1.0 - row as f32 * 0.4]);
        }
    }

    let mut indices = Vec::new();
    let stride = columns + 1;
    for row in 0..rows {
        for column in 0..columns {
            let far_left = row * stride + column;
            let near_left = far_left + stride;
            indices.extend([far_left, near_left, near_left + 1]);
            indices.extend([far_left, near_left + 1, far_left + 1]);
        }
    }
    flat_mesh(positions, uvs, indices)
}

/// One quad over the patch, its texture repeating every `1 / repeat` tiles.
/// Port of `DrawWaterPatch`, which draws two of these with different
/// scrolling.
fn quad_mesh(patch: &Patch, repeat: f32) -> Mesh {
    let half = Vec2::new(f32::from(patch.width), f32::from(patch.depth)) * (TILE_SIZE / 2.0);
    let (u, v) = (
        f32::from(patch.width) * repeat,
        f32::from(patch.depth) * repeat,
    );
    let positions = vec![
        [-half.x, 0.0, -half.y],
        [-half.x, 0.0, half.y],
        [half.x, 0.0, half.y],
        [half.x, 0.0, -half.y],
    ];
    let uvs = vec![[0.0, v], [0.0, 0.0], [u, 0.0], [u, v]];
    flat_mesh(positions, uvs, vec![0, 1, 2, 0, 2, 3])
}

fn flat_mesh(positions: Vec<[f32; 3]>, uvs: Vec<[f32; 2]>, indices: Vec<u32>) -> Mesh {
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(kind: LiquidKind, width: u8, depth: u8) -> Patch {
        Patch {
            kind,
            center: Vec3::new(1000.0, 50.0, 2000.0),
            width,
            depth,
            opacity: 1.0,
            tesselate: true,
        }
    }

    #[test]
    fn items_snap_to_tile_centres() {
        assert_eq!(tile_center(Vec2::new(321.0, 159.0)), Vec2::new(400.0, 80.0));
    }

    #[test]
    fn water_grids_have_a_vertex_every_two_tiles() {
        let mesh = grid_mesh(&patch(LiquidKind::Water, 4, 6), 2);
        assert_eq!(mesh.count_vertices(), 3 * 4);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        assert_eq!(indices.len(), 2 * 3 * 2 * 3);
    }

    #[test]
    fn only_inner_vertices_are_jittered_and_triangles_face_up() {
        let mesh = grid_mesh(&patch(LiquidKind::Slime, 3, 3), 1);
        let Some(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            panic!("no positions");
        };
        // The corner stays on the patch's edge.
        assert_eq!(positions[0], [-240.0, 0.0, -240.0]);
        // An inner vertex moves.
        assert_ne!(positions[5], [-80.0, 0.0, -80.0]);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        let p = |i: u32| Vec3::from(positions[i as usize]);
        let normal = (p(indices[1]) - p(indices[0])).cross(p(indices[2]) - p(indices[0]));
        assert!(normal.y > 0.0);
    }
}
