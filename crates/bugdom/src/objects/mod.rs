//! Objects built from the level's model files (`MakeNewDisplayGroupObject`),
//! and the material that lights them like the original.
//!
//! An object's root entity holds its gameplay state and an unrotated, unit
//! scale transform, so that its collision boxes line up with its collider.
//! The model hangs below it on a child entity with the object's rotation
//! and scale.

mod shadow;

use bevy::asset::{embedded_asset, embedded_path};
use bevy::ecs::system::SystemParam;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub use shadow::{Shadow, ShadowOf, Shadows, attach_shadow};

use crate::assets::model::Model;
use crate::level::{AMBIENT_BRIGHTNESS, CurrentLevel, FILL_BRIGHTNESS};
use crate::state::{AppState, LevelAssets};

pub struct ObjectsPlugin;

impl Plugin for ObjectsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "object.wgsl");
        app.add_plugins(MaterialPlugin::<ObjectMaterial>::default())
            .init_resource::<ObjectMaterialCache>()
            .add_systems(OnExit(AppState::InGame), clear_material_cache)
            .add_systems(Update, light_skeletons.run_if(in_state(AppState::InGame)))
            .add_plugins(shadow::plugin);
    }
}

pub type ObjectMaterial = ExtendedMaterial<StandardMaterial, ObjectShading>;

/// The level's lights, applied the way the original's renderer does.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ObjectShading {
    #[uniform(100)]
    pub lighting: ObjectLighting,
}

#[derive(ShaderType, Debug, Clone, Copy, PartialEq)]
pub struct ObjectLighting {
    pub ambient: Vec4,
    pub colors: [Vec4; 2],
    pub directions: [Vec4; 2],
    pub lit: f32,
    /// 1 to convert alpha so that blending black darkens as in display
    /// space.
    pub display_alpha: f32,
}

impl MaterialExtension for ObjectShading {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(
            bevy::asset::AssetPath::from_path_buf(embedded_path!("object.wgsl"))
                .with_source("embedded"),
        )
    }
}

impl ObjectLighting {
    /// The lights `InitArea` sets up (original/src/System/Main.c), as
    /// `QD3D_SetupLights` hands them to OpenGL (original/src/QD3D/QD3D_Support.c).
    pub fn for_level(level: CurrentLevel, shading: Shading) -> Self {
        let settings = level.def().settings();
        let [ambient, fill0, fill1] = settings.light_colors.map(Vec3::from_array);
        let [direction0, direction1] = settings.fill_directions();
        Self {
            ambient: (ambient * AMBIENT_BRIGHTNESS).extend(1.0),
            colors: [
                (fill0 * FILL_BRIGHTNESS[0]).extend(1.0),
                (fill1 * FILL_BRIGHTNESS[1]).extend(1.0),
            ],
            // The directions say where the light goes; shading needs the
            // way back to the light.
            directions: [(-direction0).extend(0.0), (-direction1).extend(0.0)],
            lit: if shading == Shading::Lit { 1.0 } else { 0.0 },
            display_alpha: if shading == Shading::Shadow { 1.0 } else { 0.0 },
        }
    }
}

/// How an object is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shading {
    /// Lit by the level's lights.
    Lit,
    /// Its plain colours (`STATUS_BIT_NULLSHADER`).
    Unlit,
    /// Unlit, and darkening what is behind it as much as the original's
    /// blending did. The original blends display values; Bevy blends
    /// linear ones, which makes a dark, partly transparent surface look
    /// much fainter. For black this is corrected exactly.
    Shadow,
}

/// The four model files a level loads, in the order of
/// [`LevelAssets::models`] (`MODEL_GROUP_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelFile {
    Global1,
    Global2,
    /// `MODEL_GROUP_LEVELSPECIFIC`
    Level1,
    /// `MODEL_GROUP_LEVELSPECIFIC2`
    Level2,
}

/// One object type in a model file (`group` and `type` of
/// `gNewObjectDefinition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub file: ModelFile,
    pub object: usize,
}

impl ModelRef {
    pub const fn new(file: ModelFile, object: usize) -> Self {
        Self { file, object }
    }
}

/// The model entity under an object's root, which carries its rotation and
/// scale.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ObjectModel;

/// Object materials made from the model files' materials, so that each is
/// made once per level.
#[derive(Resource, Debug, Default)]
pub struct ObjectMaterialCache(
    HashMap<(AssetId<StandardMaterial>, Shading, u32), Handle<ObjectMaterial>>,
);

/// Spawns models from the level's model files.
#[derive(SystemParam)]
pub struct ModelSpawner<'w> {
    level: Res<'w, CurrentLevel>,
    level_assets: Res<'w, LevelAssets>,
    models: Res<'w, Assets<Model>>,
    standard: Res<'w, Assets<StandardMaterial>>,
    materials: ResMut<'w, Assets<ObjectMaterial>>,
    cache: ResMut<'w, ObjectMaterialCache>,
}

impl ModelSpawner<'_> {
    /// Spawns `model` as a child of `parent`, with the given rotation and
    /// scale. Returns the model entity, or `None` if the model file has no
    /// such object.
    pub fn spawn(
        &mut self,
        commands: &mut Commands,
        parent: Entity,
        model: ModelRef,
        shading: Shading,
        transform: Transform,
    ) -> Option<Entity> {
        self.spawn_with_opacity(commands, parent, model, shading, 1.0, transform)
    }

    /// [`Self::spawn`], with every part made partly transparent
    /// (`MakeObjectTransparent`).
    pub fn spawn_with_opacity(
        &mut self,
        commands: &mut Commands,
        parent: Entity,
        model: ModelRef,
        shading: Shading,
        opacity: f32,
        transform: Transform,
    ) -> Option<Entity> {
        let handle = self.level_assets.models.get(model.file as usize)?;
        let file = self.models.get(handle)?;
        let Some(group) = file.groups.get(model.object) else {
            error!("{model:?} is not in the level's model files");
            return None;
        };
        let sources: Vec<_> = group
            .parts
            .iter()
            .filter_map(|&i| file.parts.get(i))
            .map(|part| (part.mesh.clone(), part.material.clone()))
            .collect();
        let parts: Vec<_> = sources
            .into_iter()
            .map(|(mesh, material)| (mesh, self.material(&material, shading, opacity)))
            .collect();
        let entity = commands
            .spawn((
                ObjectModel,
                transform,
                Visibility::default(),
                ChildOf(parent),
            ))
            .id();
        for (mesh, material) in parts {
            commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), ChildOf(entity)));
        }
        Some(entity)
    }

    /// The object material for one of the model files' materials.
    fn material(
        &mut self,
        source: &Handle<StandardMaterial>,
        shading: Shading,
        opacity: f32,
    ) -> Handle<ObjectMaterial> {
        let key = (source.id(), shading, opacity.to_bits());
        if let Some(handle) = self.cache.0.get(&key) {
            return handle.clone();
        }
        let mut base = self.standard.get(source).cloned().unwrap_or_default();
        if opacity < 1.0 {
            base.base_color.set_alpha(base.base_color.alpha() * opacity);
            base.alpha_mode = AlphaMode::Blend;
        }
        let handle = self
            .materials
            .add(object_material(base, *self.level, shading));
        self.cache.0.insert(key, handle.clone());
        handle
    }
}

fn object_material(
    base: StandardMaterial,
    level: CurrentLevel,
    shading: Shading,
) -> ObjectMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            // The extension does the lighting.
            unlit: true,
            ..base
        },
        extension: ObjectShading {
            lighting: ObjectLighting::for_level(level, shading),
        },
    }
}

/// Gives skinned models (the player and other skeletons) the object
/// material once their rig is spawned.
fn light_skeletons(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    standard: Res<Assets<StandardMaterial>>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
    mut cache: ResMut<ObjectMaterialCache>,
    meshes: Query<(Entity, &MeshMaterial3d<StandardMaterial>), With<SkinnedMesh>>,
) {
    for (entity, source) in &meshes {
        let key = (source.id(), Shading::Lit, 1.0f32.to_bits());
        let handle = cache
            .0
            .entry(key)
            .or_insert_with(|| {
                let base = standard.get(&source.0).cloned().unwrap_or_default();
                materials.add(object_material(base, *level, Shading::Lit))
            })
            .clone();
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(handle));
    }
}

fn clear_material_cache(mut cache: ResMut<ObjectMaterialCache>) {
    cache.0.clear();
}
