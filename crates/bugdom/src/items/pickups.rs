//! Pickups: the nut and its contents, powerups, the ladybug cage, and
//! opening doors with keys.
//!
//! Port of the nut, powerup and lawn door parts of
//! original/src/Items/Triggers.c and the ladybug bonus of
//! original/src/Items/Triggers2.c.
//!
//! The cage bursts into shards (`QD3D_ExplodeGeometry`); the nut's shell
//! doesn't yet, it just vanishes.

mod door;
mod ladybug;
mod nut;

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::render::render_resource::Face;

use super::ItemSystems;
use crate::objects::ObjectMaterial;
use crate::state::AppState;
use crate::terrain::TerrainSystems;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DetonatorsBlown>()
        .add_systems(
            OnEnter(AppState::InGame),
            reset_detonators
                .after(TerrainSystems::Spawn)
                .before(ItemSystems::Window),
        )
        .add_systems(
            Update,
            apply_material_overrides.run_if(in_state(AppState::InGame)),
        )
        .add_plugins((nut::plugin, ladybug::plugin, door::plugin));
}

/// A nut cracked open with a tick inside. The tick's plugin spawns it
/// (`MakeTickEnemy`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct SpawnTick {
    /// Where the nut was.
    pub position: Vec3,
}

/// A nut cracked open with the buddy bug inside (`CreateMyBuddy`). The
/// buddy isn't ported yet, so nothing answers it.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct SpawnBuddy {
    /// The player whose buddy it is.
    pub player: Entity,
    /// Where the nut was.
    pub position: Vec3,
}

/// Which detonators on the hive levels have been set off
/// (`gDetonatorBlown`), by ID. Nuts that belong to a blown detonator burst
/// and don't come back. The detonator's own plugin inserts into it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct DetonatorsBlown(pub HashSet<u8>);

impl DetonatorsBlown {
    pub fn is_blown(&self, id: u8) -> bool {
        self.0.contains(&id)
    }
}

/// Port of `InitDetonators` (original/src/Items/Triggers.c), which
/// `InitItemsManager` calls at the start of each level.
fn reset_detonators(mut blown: ResMut<DetonatorsBlown>) {
    blown.0.clear();
}

/// Changes how the meshes below an entity are drawn: their opacity
/// (`MakeObjectTransparent`), additive blending (`STATUS_BIT_GLOW`) and
/// drawing back faces (`STATUS_BIT_KEEPBACKFACES`). The meshes get
/// materials of their own the first time, so other objects of the same
/// model are left alone.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct MaterialOverride {
    opacity: f32,
    glow: bool,
    double_sided: bool,
}

impl Default for MaterialOverride {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            glow: false,
            double_sided: false,
        }
    }
}

/// A mesh whose material belongs to it alone, with what the model gave it.
#[derive(Component, Debug, Clone, Copy)]
struct OwnMaterial {
    alpha: f32,
    alpha_mode: AlphaMode,
    cull_mode: Option<Face>,
    double_sided: bool,
    applied: MaterialOverride,
}

/// Applies each [`MaterialOverride`] to the meshes below its entity.
fn apply_material_overrides(
    mut commands: Commands,
    overrides: Query<(Entity, &MaterialOverride)>,
    children: Query<&Children>,
    mut meshes: Query<(
        &mut MeshMaterial3d<ObjectMaterial>,
        Option<&mut OwnMaterial>,
    )>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    for (root, settings) in &overrides {
        for entity in children.iter_descendants(root) {
            let Ok((mut handle, own)) = meshes.get_mut(entity) else {
                continue;
            };
            match own {
                Some(mut own) => {
                    if own.applied == *settings {
                        continue;
                    }
                    own.applied = *settings;
                    if let Some(mut material) = materials.get_mut(&handle.0) {
                        apply_override(&mut material, &own);
                    }
                }
                None => {
                    let Some(mut material) = materials.get(&handle.0).cloned() else {
                        continue;
                    };
                    let own = OwnMaterial {
                        alpha: material.base.base_color.alpha(),
                        alpha_mode: material.base.alpha_mode,
                        cull_mode: material.base.cull_mode,
                        double_sided: material.base.double_sided,
                        applied: *settings,
                    };
                    apply_override(&mut material, &own);
                    handle.0 = materials.add(material);
                    commands.entity(entity).insert(own);
                }
            }
        }
    }
}

fn apply_override(material: &mut ObjectMaterial, own: &OwnMaterial) {
    let settings = own.applied;
    let base = &mut material.base;
    base.base_color.set_alpha(own.alpha * settings.opacity);
    base.alpha_mode = if settings.glow {
        AlphaMode::Add
    } else if settings.opacity < 1.0 {
        AlphaMode::Blend
    } else {
        own.alpha_mode
    };
    if settings.double_sided {
        base.cull_mode = None;
        base.double_sided = true;
    } else {
        base.cull_mode = own.cull_mode;
        base.double_sided = own.double_sided;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detonators_reset_at_level_start() {
        let mut world = World::new();
        world.init_resource::<DetonatorsBlown>();
        world.resource_mut::<DetonatorsBlown>().0.insert(7);
        assert!(world.resource::<DetonatorsBlown>().is_blown(7));
        assert!(!world.resource::<DetonatorsBlown>().is_blown(6));
        bevy::ecs::system::RunSystemOnce::run_system_once(&mut world, reset_detonators)
            .expect("the system runs");
        assert!(!world.resource::<DetonatorsBlown>().is_blown(7));
    }
}
