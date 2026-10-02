//! Fences: textured walls along the polylines in the terrain file, and the
//! circle-against-segment collision that keeps movers on their side.
//!
//! Port of original/src/Terrain/Fences.c. The meshes are built once when
//! the level starts instead of every frame. Fences fade with distance in the
//! original (`gDoAutoFade`); here fog hides them instead.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bugdom_formats::terrain::Fence as FenceDef;

use crate::assets::image::black_to_transparent;
use crate::assets::terrain::TerrainAsset;
use crate::items::{ItemSystems, ItemWindow};
use crate::level::{CurrentLevel, LevelType};
use crate::math::quick_distance;
use crate::state::{AppState, LevelAssets};
use crate::terrain::{LayerKind, MAP_TO_WORLD, TerrainMap, TerrainSystems};

pub struct FencePlugin;

impl Plugin for FencePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(AppState::InGame),
            spawn_fences.after(TerrainSystems::Spawn),
        )
        .add_systems(
            FixedUpdate,
            update_fence_visibility
                .after(ItemSystems::Window)
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// The fence types (`FENCE_TYPE_*`), which pick the texture and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FenceKind {
    Thorn,
    Wheat,
    Grass,
    Forest2,
    Night,
    Pond,
    /// Hangs from the ceiling and isn't solid.
    Moss,
    Wood,
    Hive,
}

impl FenceKind {
    const ALL: [Self; 9] = [
        Self::Thorn,
        Self::Wheat,
        Self::Grass,
        Self::Forest2,
        Self::Night,
        Self::Pond,
        Self::Moss,
        Self::Wood,
        Self::Hive,
    ];

    pub fn from_index(index: u16) -> Option<Self> {
        Self::ALL.get(usize::from(index)).copied()
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Height in world units (`gFenceHeight`).
    fn height(self) -> f32 {
        [
            600.0, 1000.0, 1000.0, 500.0, 1000.0, 2000.0, 200.0, 6000.0, 1200.0,
        ][self.index()]
    }

    /// Texture repeats per world unit along the fence (`gFenceTextureW`).
    fn texture_scale(self) -> f32 {
        match self {
            Self::Thorn => 1.0 / 600.0,
            Self::Wood => 1.0 / 1000.0,
            Self::Hive => 1.0 / 1200.0,
            _ => 1.0 / 500.0,
        }
    }

    /// Drawn without lighting (`gFenceUsesNullShader`).
    fn unlit(self) -> bool {
        self != Self::Moss
    }

    /// The texture file (`Images/Textures/200n.tga`, loaded in `PrimeFences`).
    pub fn texture_path(self) -> String {
        format!("Images/Textures/{}.tga", 2000 + self.index())
    }

    /// The fence types a level type has textures for (`gFenceOnThisLevel`).
    pub fn on_level(level: LevelType) -> &'static [Self] {
        match level {
            LevelType::Lawn => &[Self::Thorn, Self::Grass],
            LevelType::Pond => &[Self::Grass, Self::Pond],
            LevelType::Forest => &[Self::Wheat, Self::Forest2, Self::Wood],
            LevelType::Hive => &[Self::Hive],
            LevelType::Night => &[Self::Night],
            LevelType::AntHill => &[Self::Moss],
        }
    }
}

/// How far fences sink into the ground (`FENCE_SINK_FACTOR`).
const FENCE_SINK: f32 = 40.0;

/// One fence, in world units (`FenceDefType` after `PrimeFences`).
#[derive(Debug, Clone, PartialEq)]
pub struct Fence {
    pub kind: FenceKind,
    pub nubs: Vec<Vec2>,
    /// Direction of each section, normalised (`sectionVectors`).
    sections: Vec<Vec2>,
    /// Bounds in x and z.
    min: Vec2,
    max: Vec2,
}

impl Fence {
    fn new(def: &FenceDef) -> Option<Self> {
        let Some(kind) = FenceKind::from_index(def.kind) else {
            warn!("Skipping a fence of unknown type {}", def.kind);
            return None;
        };
        if def.nubs.len() < 2 {
            warn!("Skipping a fence with {} nubs", def.nubs.len());
            return None;
        }
        let nubs: Vec<Vec2> = def
            .nubs
            .iter()
            .map(|n| Vec2::new(n.x as f32, n.z as f32) * MAP_TO_WORLD)
            .collect();
        let sections = nubs
            .windows(2)
            .map(|pair| fast_normalize(pair[1] - pair[0]))
            .collect();
        let bounds = def.bounds;
        Some(Self {
            kind,
            nubs,
            sections,
            min: Vec2::new(f32::from(bounds.left), f32::from(bounds.top)) * MAP_TO_WORLD,
            max: Vec2::new(f32::from(bounds.right), f32::from(bounds.bottom)) * MAP_TO_WORLD,
        })
    }
}

/// The level's fences, and which of them are near enough to collide with.
#[derive(Resource, Debug, Clone, Default)]
pub struct Fences {
    pub fences: Vec<Fence>,
    /// Whether each fence has a nub in the active supertiles
    /// (`gIsFenceVisible`). Only those collide.
    active: Vec<bool>,
}

impl Fences {
    pub fn new(defs: &[FenceDef]) -> Self {
        let fences: Vec<Fence> = defs.iter().filter_map(Fence::new).collect();
        let active = vec![false; fences.len()];
        Self { fences, active }
    }

    /// Pushes a circle moving from `old` to `coord` back out of any fence
    /// it crossed, bouncing its velocity off the fence. `bottom` is the
    /// height of the mover's feet, for fences low enough to jump over.
    ///
    /// Port of `DoFenceCollision` (original/src/Terrain/Fences.c).
    pub fn collide(
        &self,
        map: &TerrainMap,
        old: Vec2,
        coord: &mut Vec3,
        velocity: &mut Vec3,
        radius: f32,
        bottom: f32,
    ) {
        let (old_x, old_z) = (f64::from(old.x), f64::from(old.y));
        let mut new_x = f64::from(coord.x);
        let mut new_z = f64::from(coord.z);
        let radius64 = f64::from(radius);
        // Set by the first fence that can be jumped and never cleared, so
        // every later fence can be jumped too. Kept from the original.
        let mut let_go_over = false;

        for (fence, _) in self.fences.iter().zip(&self.active).filter(|(_, a)| **a) {
            match fence.kind {
                FenceKind::Wheat | FenceKind::Forest2 => let_go_over = true,
                FenceKind::Moss => continue,
                _ => {}
            }

            // Skip fences whose bounds both ends of the motion miss.
            let (old32, new32) = (
                Vec2::new(old_x as f32, old_z as f32),
                Vec2::new(new_x as f32, new_z as f32),
            );
            let min = fence.min - Vec2::splat(radius);
            let max = fence.max + Vec2::splat(radius);
            if (old32.x < min.x && new32.x < min.x)
                || (old32.x > max.x && new32.x > max.x)
                || (old32.y < min.y && new32.y < min.y)
                || (old32.y > max.y && new32.y > max.y)
            {
                continue;
            }

            let mut rescans = 0;
            let mut i = 0;
            while i < fence.sections.len() {
                let from = fence.nubs[i];
                let to = fence.nubs[i + 1];

                if let_go_over {
                    let top = map.floor_height(from.x, from.y) + fence.kind.height();
                    if bottom >= top {
                        i += 1;
                        continue;
                    }
                }

                // The normal on the mover's side, so that the point of the
                // circle nearest the fence leads the motion.
                let normal = ray_normal(fence.sections[i], from, old32);
                let to_x = new_x - f64::from(normal.x) * radius64;
                let to_z = new_z - f64::from(normal.y) * radius64;
                let hit = intersect_segments(
                    [old_x, old_z, to_x, to_z],
                    [
                        f64::from(from.x),
                        f64::from(from.y),
                        f64::from(to.x),
                        f64::from(to.y),
                    ],
                );

                if let Some((x, z)) = hit {
                    // Back off until the circle just clears the fence.
                    coord.x =
                        (x + f64::from(normal.x) * radius64 + f64::from(normal.x) * 2.0) as f32;
                    coord.z =
                        (z + f64::from(normal.y) * radius64 + f64::from(normal.y) * 2.0) as f32;
                    bounce(velocity, normal);
                    new_x = f64::from(coord.x);
                    new_z = f64::from(coord.z);
                    rescans += 1;
                    if rescans < 5 {
                        i = 0;
                        continue;
                    }
                } else {
                    // The test above can miss the tip of a /\ corner, so an
                    // end of the section inside the circle also counts.
                    let new = Vec2::new(new_x as f32, new_z as f32);
                    if quick_distance(from, new) <= radius || quick_distance(to, new) <= radius {
                        coord.x = old.x;
                        coord.z = old.y;
                        bounce(velocity, normal);
                        return;
                    }
                }
                i += 1;
            }
        }
    }
}

/// Reflects the horizontal velocity off a fence, losing a fifth of it.
fn bounce(velocity: &mut Vec3, normal: Vec2) {
    let reflected = reflect(velocity.xz(), normal) * 0.8;
    velocity.x = reflected.x;
    velocity.z = reflected.y;
}

/// Port of `FastNormalizeVector2D` (original/src/QD3D/3DMath.c).
fn fast_normalize(v: Vec2) -> Vec2 {
    if v == Vec2::ZERO {
        return Vec2::ZERO;
    }
    v * (1.0 / (v.length() + f32::MIN_POSITIVE))
}

/// The normal of a ray that points toward `point`.
/// Port of `CalcRayNormal2D` (original/src/QD3D/3DMath.c).
fn ray_normal(direction: Vec2, origin: Vec2, point: Vec2) -> Vec2 {
    let toward = fast_normalize(point - origin);
    let cross = -(toward.x * direction.y - direction.x * toward.y);
    fast_normalize(Vec2::new(-(cross * direction.y), direction.x * cross))
}

/// Reflects a vector about a unit normal, keeping its length.
/// Port of `ReflectVector2D` (original/src/QD3D/3DMath.c).
fn reflect(v: Vec2, normal: Vec2) -> Vec2 {
    let length = v.length();
    let unit = if length != 0.0 {
        v / length
    } else {
        Vec2::ZERO
    };
    let twice_dot = 2.0 * normal.dot(unit);
    fast_normalize(normal * twice_dot - unit) * -length
}

/// Where two segments, each `[x1, y1, x2, y2]`, cross, if they do.
///
/// Port of `IntersectLineSegments` (original/src/QD3D/3DMath.c). Its side
/// tests truncate to whole numbers, so a point less than one unit (in the
/// line equation's scale) from a line counts as on it; that is kept.
fn intersect_segments(a: [f64; 4], b: [f64; 4]) -> Option<(f64, f64)> {
    let [x1, y1, x2, y2] = a;
    let [x3, y3, x4, y4] = b;
    let same_side = |r1: i64, r2: i64| r1 != 0 && r2 != 0 && (r1 < 0) == (r2 < 0);

    let a1 = y2 - y1;
    let b1 = x1 - x2;
    let c1 = x2 * y1 - x1 * y2;
    let r3 = (a1 * x3 + b1 * y3 + c1) as i64;
    let r4 = (a1 * x4 + b1 * y4 + c1) as i64;
    if same_side(r3, r4) {
        return None;
    }

    let a2 = y4 - y3;
    let b2 = x3 - x4;
    let c2 = x4 * y3 - x3 * y4;
    let r1 = (a2 * x1 + b2 * y1 + c2) as i64;
    let r2 = (a2 * x2 + b2 * y2 + c2) as i64;
    if same_side(r1, r2) {
        return None;
    }

    let denom = a1 * b2 - a2 * b1;
    if denom == 0.0 {
        // Collinear.
        return Some((x1, y1));
    }
    // Rounds rather than truncates, as the original's integer version did.
    let offset = denom.abs() * 0.5;
    let round = |num: f64| {
        if num < 0.0 {
            num - offset
        } else {
            num + offset
        }
    };
    Some((
        round(b1 * c2 - b2 * c1) / denom,
        round(a2 * c1 - a1 * c2) / denom,
    ))
}

/// Marks the fences that have a nub in an active supertile.
/// Port of the visibility test in `DrawFences`.
fn update_fence_visibility(window: Res<ItemWindow>, mut fences: ResMut<Fences>) {
    let Fences { fences, active } = &mut *fences;
    for (fence, active) in fences.iter().zip(active.iter_mut()) {
        *active = fence.nubs.iter().any(|nub| window.contains_point(*nub));
    }
}

/// Builds the fence meshes and the [`Fences`] resource.
/// Port of `PrimeFences` and `SubmitFence` (original/src/Terrain/Fences.c).
fn spawn_fences(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    level_assets: Res<LevelAssets>,
    terrains: Res<Assets<TerrainAsset>>,
    map: Res<TerrainMap>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(terrain) = terrains.get(&level_assets.terrain) else {
        return;
    };
    let fences = Fences::new(&terrain.fences);
    let level_type = level.def().level_type;

    let mut fence_materials = Vec::new();
    for (kind, texture) in &level_assets.fence_textures {
        if let Some(mut image) = images.get_mut(texture) {
            if let Some(data) = image.data.as_mut() {
                black_to_transparent(data);
            }
            // Clamped vertically so the top edge doesn't bleed.
            if let ImageSampler::Descriptor(sampler) = &mut image.sampler {
                sampler.address_mode_v = ImageAddressMode::ClampToEdge;
            }
        }
        let material = materials.add(StandardMaterial {
            base_color_texture: Some(texture.clone()),
            alpha_mode: AlphaMode::Mask(0.5),
            unlit: kind.unlit(),
            double_sided: true,
            cull_mode: None,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        });
        fence_materials.push((*kind, material));
    }

    for (index, fence) in fences.fences.iter().enumerate() {
        let Some((_, material)) = fence_materials.iter().find(|(k, _)| *k == fence.kind) else {
            warn!(
                "Fence {index} is of type {:?}, which {level_type:?} levels don't have",
                fence.kind
            );
            continue;
        };
        commands.spawn((
            Name::new(format!("Fence {index}")),
            Mesh3d(meshes.add(fence_mesh(fence, &map))),
            MeshMaterial3d(material.clone()),
            DespawnOnExit(AppState::InGame),
        ));
    }
    commands.insert_resource(fences);
}

/// A fence's wall: a bottom and top vertex at every nub.
fn fence_mesh(fence: &Fence, map: &TerrainMap) -> Mesh {
    let kind = fence.kind;
    let mut positions = Vec::with_capacity(fence.nubs.len() * 2);
    let mut uvs = Vec::with_capacity(fence.nubs.len() * 2);
    let mut u = 0.0;
    let mut previous: Option<Vec3> = None;
    for nub in &fence.nubs {
        let (x, z) = (nub.x, nub.y);
        let (bottom, top) = match kind {
            FenceKind::Moss => {
                let y = map.height_at(x, z, LayerKind::Ceiling).0 + FENCE_SINK;
                (y, y - kind.height())
            }
            FenceKind::Wood => (-400.0, -400.0 + kind.height()),
            FenceKind::Hive => (
                map.floor_height(x, z) - FENCE_SINK,
                map.height_at(x, z, LayerKind::Ceiling).0 + FENCE_SINK,
            ),
            _ => {
                let y = map.floor_height(x, z) - FENCE_SINK;
                (y, y + kind.height())
            }
        };
        let base = Vec3::new(x, bottom, z);
        if let Some(previous) = previous {
            u += base.distance(previous) * kind.texture_scale();
        }
        previous = Some(base);
        positions.push(base.to_array());
        positions.push([x, top, z]);
        uvs.push([u, 1.0]);
        uvs.push([u, 0.0]);
    }

    // Face normals summed at each vertex, flat in y; only lit fences use them.
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for (i, pair) in fence.nubs.windows(2).enumerate() {
        let face = Vec3::new(-(pair[1].y - pair[0].y), 0.0, -(pair[1].x - pair[0].x));
        for normal in &mut normals[i * 2..i * 2 + 4] {
            *normal += face;
        }
    }
    let normals: Vec<[f32; 3]> = normals
        .into_iter()
        .map(|n| n.normalize_or(Vec3::Y).to_array())
        .collect();

    let mut indices = Vec::with_capacity((fence.nubs.len() - 1) * 6);
    for j in (0..(fence.nubs.len() as u32 - 1) * 2).step_by(2) {
        indices.extend([1 + j, j, 3 + j, 3 + j, j, 2 + j]);
    }

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

    #[test]
    fn crossing_segments_meet_where_expected() {
        let hit = intersect_segments([0.0, -100.0, 0.0, 100.0], [-100.0, 0.0, 100.0, 0.0]);
        let (x, z) = hit.expect("the segments cross");
        assert!(x.abs() < 1.0 && z.abs() < 1.0);
        assert!(intersect_segments([0.0, 10.0, 0.0, 100.0], [-100.0, 0.0, 100.0, 0.0]).is_none());
    }

    #[test]
    fn reflecting_keeps_the_length() {
        let v = reflect(Vec2::new(3.0, -4.0), Vec2::Y);
        assert!(v.abs_diff_eq(Vec2::new(3.0, 4.0), 1e-4), "{v}");
    }

    #[test]
    fn walking_into_a_fence_stops_on_its_side() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let fence = Fence {
            kind: FenceKind::Thorn,
            nubs: vec![Vec2::new(12000.0, 15000.0), Vec2::new(13000.0, 15000.0)],
            sections: vec![Vec2::X],
            min: Vec2::new(12000.0, 15000.0),
            max: Vec2::new(13000.0, 15000.0),
        };
        let fences = Fences {
            fences: vec![fence],
            active: vec![true],
        };
        let old = Vec2::new(12500.0, 15060.0);
        let mut coord = Vec3::new(12500.0, 0.0, 15030.0);
        let mut velocity = Vec3::new(0.0, 0.0, -600.0);
        fences.collide(&map, old, &mut coord, &mut velocity, 42.0, 0.0);
        assert!(coord.z >= 15000.0 + 42.0, "ended at {coord}");
        assert!(velocity.z > 0.0, "bounced to {velocity}");
    }
}
