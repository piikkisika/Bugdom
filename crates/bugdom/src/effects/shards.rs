//! Shards: an object bursting into its own triangles, which tumble, fall
//! and shrink away.
//!
//! Port of `QD3D_ExplodeGeometry`, `ExplodeTriMesh` and `QD3D_MoveShards`
//! (original/src/QD3D/QD3D_Geometry.c).
//!
//! An owner bursts an object with [`explode_geometry`], a command queued
//! before the object is despawned, so that it still finds the object's
//! meshes and pose. A skinned mesh is skinned on the CPU at that moment, as
//! the original's skeleton vertices are already transformed.

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::Command;
use bevy::math::Affine3A;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::Face;

use crate::math::GameRandom;
use crate::objects::ObjectMaterial;
use crate::state::AppState;
use crate::terrain::TerrainMap;

/// The most shards at once (`MAX_SHARDS`).
pub const MAX_SHARDS: usize = 80;
/// Gravity on shards, in units per second squared (`1700 / 3`, and
/// `1700 / 2` with [`ShardMode::HEAVY_GRAVITY`]).
const SHARD_GRAVITY: f32 = 1700.0 / 3.0;
const SHARD_HEAVY_GRAVITY: f32 = 1700.0 / 2.0;
/// The spread of a shard's spin, in radians per second, across the whole
/// width.
const SHARD_SPIN_SPREAD: f32 = 4.0;
/// How much of its upward push an upthrust adds, per unit of the force.
const SHARD_UPTHRUST: f32 = 1.5;
/// What a bounce keeps of a shard's velocity, up and across.
const SHARD_BOUNCE_UP: f32 = -0.5;
const SHARD_BOUNCE_ACROSS: f32 = 0.9;

/// How shards behave (`SHARD_MODE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShardMode(u8);

impl ShardMode {
    pub const NONE: Self = Self(0);
    /// Bounce on the floor rather than vanish there.
    pub const BOUNCE: Self = Self(1);
    /// Burst upward.
    pub const UPTHRUST: Self = Self(1 << 1);
    pub const HEAVY_GRAVITY: Self = Self(1 << 2);
    /// Drawn with their plain colours (`SHARD_MODE_NULLSHADER`).
    pub const NULL_SHADER: Self = Self(1 << 3);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for ShardMode {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// How an object bursts: the arguments of `QD3D_ExplodeGeometry`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Explosion {
    /// The spread of the shards' speed, in units per second, across the
    /// whole width.
    pub force: f32,
    pub mode: ShardMode,
    /// Every this many triangles makes a shard.
    pub density: usize,
    /// How fast the shards shrink away, in scale per second.
    pub decay: f32,
}

/// One flying triangle.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(DespawnOnExit<AppState> = DespawnOnExit(AppState::InGame))]
pub struct Shard {
    pub velocity: Vec3,
    pub rotation: Vec3,
    pub spin: Vec3,
    pub scale: f32,
    pub decay: f32,
    pub mode: ShardMode,
}

/// Two-sided copies of the objects' materials for their shards
/// (`STATUS_BIT_KEEPBACKFACES`), made once per material and shader mode.
#[derive(Resource, Debug, Default)]
struct ShardMaterials(HashMap<(AssetId<ObjectMaterial>, bool), Handle<ObjectMaterial>>);

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ShardMaterials>()
        .add_systems(OnExit(AppState::InGame), clear_shard_materials)
        .add_systems(FixedUpdate, move_shards.run_if(in_state(AppState::InGame)));
}

fn clear_shard_materials(mut materials: ResMut<ShardMaterials>) {
    materials.0.clear();
}

/// A command that bursts `entity`'s meshes (it and its descendants) into
/// shards. Queue it before despawning the entity.
///
/// Port of `QD3D_ExplodeGeometry`.
pub fn explode_geometry(entity: Entity, explosion: Explosion) -> impl Command {
    move |world: &mut World| explode(world, entity, explosion)
}

/// A triangle of an object's mesh, in world space.
struct Triangle {
    points: [Vec3; 3],
    normals: [Vec3; 3],
    uvs: [[f32; 2]; 3],
    colors: Option<[[f32; 4]; 3]>,
    material: Handle<ObjectMaterial>,
}

fn explode(world: &mut World, entity: Entity, explosion: Explosion) {
    let mut free = MAX_SHARDS.saturating_sub(
        world
            .query_filtered::<(), With<Shard>>()
            .iter(world)
            .count(),
    );
    if free == 0 {
        return;
    }
    let triangles = collect_triangles(world, entity, explosion.density.max(1));
    let null_shader = explosion.mode.contains(ShardMode::NULL_SHADER);
    for triangle in triangles {
        if free == 0 {
            break;
        }
        free -= 1;
        let center = (triangle.points[0] + triangle.points[1] + triangle.points[2]) / 3.0;
        let material = shard_material(world, &triangle.material, null_shader);
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            triangle.points.map(|p| (p - center).to_array()).to_vec(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            triangle.normals.map(|n| n.to_array()).to_vec(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, triangle.uvs.to_vec());
        if let Some(colors) = triangle.colors {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors.to_vec());
        }
        let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
        let shard = {
            let mut random = world.resource_mut::<GameRandom>();
            let mut spread = |width: f32| (random.next_f32() - 0.5) * width;
            let mut velocity = Vec3::new(
                spread(explosion.force),
                spread(explosion.force),
                spread(explosion.force),
            );
            if explosion.mode.contains(ShardMode::UPTHRUST) {
                velocity.y += SHARD_UPTHRUST * explosion.force;
            }
            let spin = Vec3::new(
                spread(SHARD_SPIN_SPREAD),
                spread(SHARD_SPIN_SPREAD),
                spread(SHARD_SPIN_SPREAD),
            );
            Shard {
                velocity,
                rotation: Vec3::ZERO,
                spin,
                scale: 1.0,
                decay: explosion.decay,
                mode: explosion.mode,
            }
        };
        world.spawn((
            Name::new("Shard"),
            shard,
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(center),
        ));
    }
}

/// Every `density`th triangle of the entity's meshes and its descendants',
/// in world space (`ExplodeTriMesh`'s transform of the points and
/// normals).
fn collect_triangles(world: &mut World, root: Entity, density: usize) -> Vec<Triangle> {
    let mut entities = vec![root];
    let mut i = 0;
    while i < entities.len() {
        if let Some(children) = world.get::<Children>(entities[i]) {
            entities.extend(children.iter());
        }
        i += 1;
    }
    let mut triangles = Vec::new();
    for entity in entities {
        let Some(mesh) = world.get::<Mesh3d>(entity).map(|m| m.0.clone()) else {
            continue;
        };
        let Some(material) = world
            .get::<MeshMaterial3d<ObjectMaterial>>(entity)
            .map(|m| m.0.clone())
        else {
            continue;
        };
        let transform = vertex_transform(world, entity);
        let meshes = world.resource::<Assets<Mesh>>();
        let Some(mesh) = meshes.get(&mesh) else {
            continue;
        };
        let Some(transform) = transform else {
            continue;
        };
        add_mesh_triangles(mesh, &transform, &material, density, &mut triangles);
    }
    triangles
}

/// How a mesh's vertices get to the world: its global transform, or, for a
/// skinned mesh, each joint's skinning matrix.
enum VertexTransform {
    Rigid(Affine3A),
    Skinned(Vec<Mat4>),
}

fn vertex_transform(world: &World, entity: Entity) -> Option<VertexTransform> {
    if let Some(skin) = world.get::<SkinnedMesh>(entity) {
        let bindposes = world
            .resource::<Assets<SkinnedMeshInverseBindposes>>()
            .get(&skin.inverse_bindposes)?;
        let joints = skin
            .joints
            .iter()
            .zip(bindposes.iter())
            .map(|(&joint, inverse)| {
                let global = world
                    .get::<GlobalTransform>(joint)
                    .copied()
                    .unwrap_or_default();
                global.to_matrix() * *inverse
            })
            .collect();
        return Some(VertexTransform::Skinned(joints));
    }
    let global = world.get::<GlobalTransform>(entity)?;
    Some(VertexTransform::Rigid(global.affine()))
}

fn add_mesh_triangles(
    mesh: &Mesh,
    transform: &VertexTransform,
    material: &Handle<ObjectMaterial>,
    density: usize,
    out: &mut Vec<Triangle>,
) {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return;
    };
    let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(normals)) => Some(normals),
        _ => None,
    };
    let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(uvs)) => Some(uvs),
        _ => None,
    };
    let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(colors)) => Some(colors),
        _ => None,
    };
    let joints = match mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX) {
        Some(VertexAttributeValues::Uint16x4(joints)) => Some(joints),
        _ => None,
    };
    let weights = match mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) {
        Some(VertexAttributeValues::Float32x4(weights)) => Some(weights),
        _ => None,
    };
    let indices: Vec<usize> = match mesh.indices() {
        Some(Indices::U16(i)) => i.iter().map(|&i| usize::from(i)).collect(),
        Some(Indices::U32(i)) => i.iter().filter_map(|&i| usize::try_from(i).ok()).collect(),
        None => (0..positions.len()).collect(),
    };

    // The matrix that takes vertex `v` to the world.
    let matrix = |v: usize| -> Mat4 {
        match transform {
            VertexTransform::Rigid(affine) => Mat4::from(*affine),
            VertexTransform::Skinned(skin) => {
                let (Some(joints), Some(weights)) = (joints, weights) else {
                    return Mat4::IDENTITY;
                };
                let (j, w) = (joints[v], weights[v]);
                (0..4)
                    .map(|k| {
                        skin.get(usize::from(j[k]))
                            .map_or(Mat4::ZERO, |m| *m * w[k])
                    })
                    .fold(Mat4::ZERO, |sum, m| sum + m)
            }
        }
    };

    for triangle in indices.chunks_exact(3).step_by(density) {
        let mut points = [Vec3::ZERO; 3];
        let mut normal_out = [Vec3::Y; 3];
        let mut uv_out = [[0.0; 2]; 3];
        let mut color_out = [[1.0; 4]; 3];
        for (k, &v) in triangle.iter().enumerate() {
            let Some(&position) = positions.get(v) else {
                return;
            };
            let m = matrix(v);
            points[k] = m.transform_point3(Vec3::from_array(position));
            if let Some(n) = normals.and_then(|n| n.get(v)) {
                normal_out[k] = m
                    .transform_vector3(Vec3::from_array(*n))
                    .normalize_or_zero();
            }
            if let Some(uv) = uvs.and_then(|u| u.get(v)) {
                uv_out[k] = *uv;
            }
            if let Some(c) = colors.and_then(|c| c.get(v)) {
                color_out[k] = *c;
            }
        }
        out.push(Triangle {
            points,
            normals: normal_out,
            uvs: uv_out,
            colors: colors.map(|_| color_out),
            material: material.clone(),
        });
    }
}

/// The two-sided copy of a material for shards, unlit with
/// [`ShardMode::NULL_SHADER`].
fn shard_material(
    world: &mut World,
    source: &Handle<ObjectMaterial>,
    null_shader: bool,
) -> Handle<ObjectMaterial> {
    let key = (source.id(), null_shader);
    if let Some(handle) = world.resource::<ShardMaterials>().0.get(&key) {
        return handle.clone();
    }
    let mut materials = world.resource_mut::<Assets<ObjectMaterial>>();
    let Some(mut material) = materials.get(source).cloned() else {
        return source.clone();
    };
    material.base.cull_mode = None::<Face>;
    material.base.double_sided = true;
    if null_shader {
        material.extension.lighting.lit = 0.0;
    }
    let handle = materials.add(material);
    world
        .resource_mut::<ShardMaterials>()
        .0
        .insert(key, handle.clone());
    handle
}

/// Spins, drops and shrinks the shards; one that reaches the floor bounces
/// or vanishes, and one that has shrunk away is gone.
///
/// Port of `QD3D_MoveShards`.
fn move_shards(
    time: Res<Time>,
    map: Option<Res<TerrainMap>>,
    mut commands: Commands,
    mut shards: Query<(Entity, &mut Shard, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut shard, mut transform) in &mut shards {
        if step_shard(&mut shard, &mut transform.translation, dt, |x, z| {
            // The original pins shards to a floor at -100 without terrain.
            map.as_ref().map_or(-100.0, |m| m.floor_height(x, z))
        }) {
            transform.rotation = Quat::from_euler(
                EulerRot::ZYX,
                shard.rotation.z,
                shard.rotation.y,
                shard.rotation.x,
            );
            transform.scale = Vec3::splat(shard.scale);
        } else {
            commands.entity(entity).despawn();
        }
    }
}

/// One tick of a shard; `false` once it is gone.
fn step_shard(
    shard: &mut Shard,
    coord: &mut Vec3,
    dt: f32,
    floor: impl Fn(f32, f32) -> f32,
) -> bool {
    shard.rotation += shard.spin * dt;
    shard.velocity.y -= dt
        * if shard.mode.contains(ShardMode::HEAVY_GRAVITY) {
            SHARD_HEAVY_GRAVITY
        } else {
            SHARD_GRAVITY
        };
    *coord += shard.velocity * dt;
    let floor = floor(coord.x, coord.z);
    if coord.y <= floor {
        if !shard.mode.contains(ShardMode::BOUNCE) {
            return false;
        }
        coord.y = floor;
        shard.velocity.y *= SHARD_BOUNCE_UP;
        shard.velocity.x *= SHARD_BOUNCE_ACROSS;
        shard.velocity.z *= SHARD_BOUNCE_ACROSS;
    }
    shard.scale -= shard.decay * dt;
    shard.scale > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shard(mode: ShardMode) -> Shard {
        Shard {
            velocity: Vec3::new(100.0, -300.0, 0.0),
            rotation: Vec3::ZERO,
            spin: Vec3::ONE,
            scale: 1.0,
            decay: 0.5,
            mode,
        }
    }

    #[test]
    fn a_shard_vanishes_on_the_floor_unless_it_bounces() {
        let mut coord = Vec3::new(0.0, 1.0, 0.0);
        let mut plain = shard(ShardMode::NONE);
        assert!(!step_shard(&mut plain, &mut coord, 0.1, |_, _| 0.0));

        let mut coord = Vec3::new(0.0, 1.0, 0.0);
        let mut bouncy = shard(ShardMode::BOUNCE);
        assert!(step_shard(&mut bouncy, &mut coord, 0.1, |_, _| 0.0));
        assert_eq!(coord.y, 0.0);
        assert!(bouncy.velocity.y > 0.0);
        assert!((bouncy.velocity.x - 90.0).abs() < 1e-3);
    }

    #[test]
    fn a_shard_shrinks_away_at_its_decay() {
        let mut coord = Vec3::new(0.0, 10_000.0, 0.0);
        let mut s = shard(ShardMode::NONE);
        s.velocity = Vec3::ZERO;
        let mut ticks = 0;
        while step_shard(&mut s, &mut coord, 1.0 / 60.0, |_, _| -1.0e9) {
            ticks += 1;
        }
        // 1 / 0.5 per second = 2 s = 120 ticks.
        assert!((119..=120).contains(&ticks), "{ticks}");
    }

    #[test]
    fn heavy_gravity_pulls_harder() {
        let (mut a, mut b) = (shard(ShardMode::NONE), shard(ShardMode::HEAVY_GRAVITY));
        let (mut ca, mut cb) = (Vec3::splat(1000.0), Vec3::splat(1000.0));
        step_shard(&mut a, &mut ca, 0.1, |_, _| 0.0);
        step_shard(&mut b, &mut cb, 0.1, |_, _| 0.0);
        assert!(b.velocity.y < a.velocity.y);
    }
}
