//! Water ripples: a ring that grows and fades on a liquid's surface.
//!
//! Port of `MakeRipple` and `MoveRipple` (original/src/Items/Effects.c).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::assets::model::Model;
use crate::level::CurrentLevel;
use crate::objects::{ModelFile, ModelRef, ObjectLighting, ObjectMaterial, ObjectShading, Shading};
use crate::state::{AppState, LevelAssets};

/// The ripple model (`GLOBAL1_MObjType_Ripple`).
const RIPPLE_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 3);
/// A new ripple's opacity (`Health = .8`).
const RIPPLE_START_OPACITY: f32 = 0.8;
/// How fast it fades, in opacity per second.
const RIPPLE_FADE_RATE: f32 = 0.8;
/// How fast it grows, in scale per second.
const RIPPLE_GROWTH_RATE: f32 = 6.0;

/// A fading ripple. Its materials are its own, so that fading one ripple
/// doesn't fade the others; they are freed with it.
#[derive(Component, Debug, Clone)]
pub struct Ripple {
    /// Its opacity, which runs down to 0 (`Health`).
    pub opacity: f32,
    /// Each part's material and the alpha the model gives it.
    materials: Vec<(Handle<ObjectMaterial>, f32)>,
}

/// What [`make_ripple`] needs.
#[derive(SystemParam)]
pub struct RippleMaker<'w> {
    level: Res<'w, CurrentLevel>,
    level_assets: Res<'w, LevelAssets>,
    models: Res<'w, Assets<Model>>,
    standard: Res<'w, Assets<StandardMaterial>>,
    materials: ResMut<'w, Assets<ObjectMaterial>>,
}

/// Spawns a ripple at `position` (the liquid's surface) with the given
/// starting scale. Returns `None` if the ripple model is missing.
///
/// Port of `MakeRipple`. `ModelSpawner` shares materials between objects
/// of the same opacity, which won't do for a fading object, so this builds
/// the model itself with materials of its own. The original draws ripples
/// after the water (`kDrawOrder_Ripples`); here Bevy's transparent sort
/// decides.
pub fn make_ripple(
    commands: &mut Commands,
    maker: &mut RippleMaker,
    position: Vec3,
    start_scale: f32,
) -> Option<Entity> {
    let handle = maker.level_assets.models.get(RIPPLE_MODEL.file as usize)?;
    let model = maker.models.get(handle)?;
    let Some(group) = model.groups.get(RIPPLE_MODEL.object) else {
        error!("{RIPPLE_MODEL:?} is not in the level's model files");
        return None;
    };
    let lighting = ObjectLighting::for_level(*maker.level, Shading::Lit);
    let mut parts = Vec::new();
    let mut materials = Vec::new();
    for part in group.parts.iter().filter_map(|&i| model.parts.get(i)) {
        let base = maker
            .standard
            .get(&part.material)
            .cloned()
            .unwrap_or_default();
        let model_alpha = base.base_color.alpha();
        let material = maker.materials.add(ObjectMaterial {
            base: StandardMaterial {
                base_color: base
                    .base_color
                    .with_alpha(model_alpha * RIPPLE_START_OPACITY),
                // `STATUS_BIT_GLOW`; blended materials don't write depth
                // (`STATUS_BIT_NOZWRITE`).
                alpha_mode: AlphaMode::Add,
                // `STATUS_BIT_NOFOG`
                fog_enabled: false,
                // The extension does the lighting.
                unlit: true,
                ..base
            },
            extension: ObjectShading { lighting },
        });
        parts.push((part.mesh.clone(), material.clone()));
        materials.push((material, model_alpha));
    }
    let root = commands
        .spawn((
            Name::new("Ripple"),
            Ripple {
                opacity: RIPPLE_START_OPACITY,
                materials,
            },
            Transform::from_translation(position).with_scale(Vec3::splat(start_scale)),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    for (mesh, material) in parts {
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), ChildOf(root)));
    }
    Some(root)
}

/// Fades and grows ripples, and removes them once they have faded.
/// Port of `MoveRipple`.
pub(super) fn move_ripples(
    time: Res<Time>,
    mut commands: Commands,
    mut materials: ResMut<Assets<ObjectMaterial>>,
    mut ripples: Query<(Entity, &mut Ripple, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut ripple, mut transform) in &mut ripples {
        ripple.opacity -= RIPPLE_FADE_RATE * dt;
        if ripple.opacity < 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        for (handle, model_alpha) in &ripple.materials {
            if let Some(mut material) = materials.get_mut(handle) {
                material
                    .base
                    .base_color
                    .set_alpha(model_alpha * ripple.opacity);
            }
        }
        transform.scale += Vec3::splat(RIPPLE_GROWTH_RATE * dt);
    }
}
