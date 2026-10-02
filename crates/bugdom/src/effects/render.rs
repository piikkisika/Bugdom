//! Drawing the particle groups: one mesh per group, rebuilt every frame
//! from camera-facing quads.
//!
//! Port of `DrawParticleGroup` and the texture loading in
//! `InitParticleSystem` (original/src/Items/Effects.c).

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageSampler};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use super::particles::{ParticleGroup, ParticleGroupId, ParticleGroups, ParticleTexture};
use crate::assets::original_path;
use crate::camera::GameCamera;
use crate::state::AppState;

/// The material of each particle texture, in [`ParticleTexture`] order.
#[derive(Resource, Debug, Clone)]
pub(super) struct ParticleMaterials {
    textures: Vec<Handle<Image>>,
    materials: Vec<Handle<StandardMaterial>>,
}

/// The mesh entity that draws one particle group.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ParticleGroupMesh(ParticleGroupId);

/// Loads the particle textures and makes their materials, once, as the
/// original loads them once (`gParticleTexturesLoaded`).
pub(super) fn load_particle_materials(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let textures: Vec<Handle<Image>> = ParticleTexture::ALL
        .iter()
        .map(|texture| assets.load(original_path(&texture.path())))
        .collect();
    let materials = textures
        .iter()
        .map(|texture| {
            materials.add(StandardMaterial {
                base_color_texture: Some(texture.clone()),
                // `STATUS_BIT_GLOW`: added to what is behind, scaled by
                // alpha (`glBlendFunc(GL_SRC_ALPHA, GL_ONE)`). Blended
                // materials don't write depth (`STATUS_BIT_NOZWRITE`).
                alpha_mode: AlphaMode::Add,
                // `STATUS_BIT_NULLSHADER`
                unlit: true,
                // `STATUS_BIT_NOFOG`
                fog_enabled: false,
                cull_mode: None,
                double_sided: true,
                ..default()
            })
        })
        .collect();
    commands.insert_resource(ParticleMaterials {
        textures,
        materials,
    });
}

/// Clamps the particle textures to their edges once they load
/// (`kRendererTextureFlags_ClampBoth`), so a quad's border doesn't pick up
/// the opposite side.
pub(super) fn clamp_particle_textures(
    mut events: MessageReader<AssetEvent<Image>>,
    particle_materials: Res<ParticleMaterials>,
    mut images: ResMut<Assets<Image>>,
) {
    for event in events.read() {
        let AssetEvent::LoadedWithDependencies { id } = event else {
            continue;
        };
        if !particle_materials.textures.iter().any(|t| t.id() == *id) {
            continue;
        }
        if let Some(mut image) = images.get_mut(*id)
            && let ImageSampler::Descriptor(sampler) = &mut image.sampler
        {
            sampler.address_mode_u = ImageAddressMode::ClampToEdge;
            sampler.address_mode_v = ImageAddressMode::ClampToEdge;
        }
    }
}

/// Keeps one mesh entity per live group and rebuilds its quads to face the
/// camera. Runs after transforms are propagated, so it sees where the
/// camera is drawn from this frame.
///
/// Port of `DrawParticleGroup`. The original also skips particles outside
/// the view frustum to save overdraw; this draws them all.
pub(super) fn draw_particle_groups(
    mut commands: Commands,
    groups: Res<ParticleGroups>,
    particle_materials: Res<ParticleMaterials>,
    cameras: Query<&GlobalTransform, With<GameCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drawn: Query<(Entity, &ParticleGroupMesh, &Mesh3d, &mut Visibility)>,
) {
    let Some(camera) = cameras.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    let mut has_mesh = HashSet::new();
    for (entity, &ParticleGroupMesh(id), mesh, mut visibility) in &mut drawn {
        let Some(group) = groups.get(id) else {
            commands.entity(entity).despawn();
            continue;
        };
        has_mesh.insert(id);
        if group.particles().is_empty() {
            // An empty mesh can't be drawn; the group goes next tick.
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        visibility.set_if_neq(Visibility::Inherited);
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            *mesh = group_mesh(group, camera);
        }
    }

    for (id, group) in groups.iter() {
        if has_mesh.contains(&id) || group.particles().is_empty() {
            continue;
        }
        let Some(material) = particle_materials
            .materials
            .get(group.desc.texture as usize)
            .cloned()
        else {
            continue;
        };
        commands.spawn((
            Name::new("Particle group"),
            ParticleGroupMesh(id),
            Mesh3d(meshes.add(group_mesh(group, camera))),
            MeshMaterial3d(material),
            Transform::default(),
            Visibility::default(),
            // The mesh moves every frame; its bounds are never worth
            // computing.
            NoFrustumCulling,
            DespawnOnExit(AppState::InGame),
        ));
    }
}

/// Two triangles per particle, in world space, facing `camera`.
fn group_mesh(group: &ParticleGroup, camera: Vec3) -> Mesh {
    let count = group.particles().len();
    let mut positions = Vec::with_capacity(count * 4);
    let mut normals = Vec::with_capacity(count * 4);
    let mut colors = Vec::with_capacity(count * 4);
    let mut uvs = Vec::with_capacity(count * 4);
    let mut indices = Vec::with_capacity(count * 6);
    for particle in group.particles() {
        let size = group.desc.base_scale * particle.scale;
        let (right, up, toward_camera) = look_at_axes(particle.position, camera);
        let base = positions.len() as u32;
        // `v[0..4]` and the UVs of `InitParticleGroup`.
        for (corner, uv) in [
            (Vec2::new(size, size), [0.0, 1.0]),
            (Vec2::new(size, -size), [0.0, 0.0]),
            (Vec2::new(-size, -size), [1.0, 0.0]),
            (Vec2::new(-size, size), [1.0, 1.0]),
        ] {
            positions.push((particle.position + right * corner.x + up * corner.y).to_array());
            normals.push(toward_camera.to_array());
            colors.push([1.0, 1.0, 1.0, particle.alpha]);
            uvs.push(uv);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

/// The x and y axes of `SetLookAtMatrixAndTranslate` with an up vector of
/// +Y, plus the direction to the camera for lighting-free normals.
///
/// Like the original, y stays world up and x is not normalised, so a
/// particle seen from above narrows rather than turning to face the camera.
fn look_at_axes(position: Vec3, camera: Vec3) -> (Vec3, Vec3, Vec3) {
    let look = fast_normalize(position - camera);
    let right = Vec3::new(look.z, 0.0, -look.x);
    (right, Vec3::Y, -look)
}

fn fast_normalize(v: Vec3) -> Vec3 {
    if v == Vec3::ZERO {
        return Vec3::ZERO;
    }
    v / (v.length() + f32::MIN_POSITIVE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::particles::ParticleGroupDesc;

    #[test]
    fn quads_face_the_camera_and_keep_the_original_uvs() {
        let mut groups = ParticleGroups::default();
        let id = groups
            .new_group(ParticleGroupDesc {
                base_scale: 10.0,
                ..default()
            })
            .unwrap();
        groups.add_particle(id, Vec3::new(0.0, 0.0, -100.0), Vec3::ZERO, 2.0, 0.5);
        let mesh = group_mesh(groups.get(id).unwrap(), Vec3::ZERO);
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .unwrap();
        // Looking down −Z, the first corner is top left.
        assert_eq!(positions[0], [-20.0, 20.0, -100.0]);
        assert_eq!(positions[2], [20.0, -20.0, -100.0]);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        assert_eq!(indices.len(), 6);
        // Wound counter-clockwise as seen from the camera.
        let p = |i: u32| Vec3::from(positions[i as usize]);
        let normal = (p(indices[1]) - p(indices[0])).cross(p(indices[2]) - p(indices[0]));
        assert!(normal.z > 0.0);
    }
}
