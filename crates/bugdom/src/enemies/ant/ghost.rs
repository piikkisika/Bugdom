//! The ghost that rises from a dead ant in the Ant Hill: a glowing ant
//! that nothing can touch, which fights on as the dead one did.
//!
//! Port of `MakeGhostAnt` (original/src/Enemies/Enemy_Ant.c).

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::items::{self, Carried};
use super::{ANT_HEAD_OFFSET, AntBrain, AntState, ant_skeleton};
use crate::enemies::{EnemyModel, EnemySkeleton, EnemySpawner, HomePosition};
use crate::physics::PreviousPosition;
use crate::skeleton::SkeletonAnimator;

/// How much of the blend from the dead ant's pose into standing happens
/// per second.
const GHOST_STAND_MORPH_RATE: f32 = 4.0;

/// What a ghost takes from the dead ant.
#[derive(Debug, Clone)]
pub struct GhostAnt {
    /// Where the body is (`gCoord`).
    pub at: Vec3,
    pub rock_thrower: bool,
    pub aggressive: bool,
    /// The body's animation, which the ghost starts from.
    pub animator: SkeletonAnimator,
}

/// Marks a ghost's model, whose parts glow (`STATUS_BIT_GLOW`).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct GhostGlow;

/// A ghost's model part that glows already.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Glowing;

/// Raises a ghost from a dead ant. It arrives when the commands are
/// applied.
pub fn make_ghost_ant(commands: &mut Commands, ghost: GhostAnt) {
    commands.run_system_cached_with(spawn_ghost_ant, ghost);
}

/// Port of `MakeGhostAnt` (original/src/Enemies/Enemy_Ant.c), with
/// `MakeAntObject`: an ant without a shadow and without collision kinds,
/// exactly where the body is, facing as `MakeEnemySkeleton` leaves it,
/// holding a new spear unless it throws rocks. Like the original, it
/// doesn't check the enemy counts, but counts as an enemy.
fn spawn_ghost_ant(In(ghost): In<GhostAnt>, mut enemies: EnemySpawner) {
    let def = EnemySkeleton {
        // "Nothing can touch these" (`CType = 0`).
        kinds: LayerMask::NONE,
        ..ant_skeleton(ghost.at.xz(), ANT_HEAD_OFFSET)
    };
    let Some(ant) = enemies.spawn(def) else {
        return;
    };
    let at = ghost.at;
    let commands = enemies.commands();
    commands.entity(ant).insert((
        Transform::from_translation(at),
        PreviousPosition(at),
        HomePosition(at),
        AntBrain {
            state: AntState::Stand,
            rock_thrower: ghost.rock_thrower,
            aggressive: ghost.aggressive,
            ..default()
        },
    ));
    if !ghost.rock_thrower {
        items::give_item(commands, ant, Carried::Spear);
    }
    commands.run_system_cached_with(haunt_ghost_model, (ant, ghost.animator));
}

/// Gives a new ghost's model the dead ant's animation, blending into
/// standing, and makes it glow.
fn haunt_ghost_model(
    In((ghost, mut animator)): In<(Entity, SkeletonAnimator)>,
    mut commands: Commands,
    ants: Query<&EnemyModel>,
) {
    let Ok(model) = ants.get(ghost) else {
        return;
    };
    animator.morph_to(AntState::Stand.anim(), GHOST_STAND_MORPH_RATE);
    commands.entity(model.0).insert((animator, GhostGlow));
}

/// Makes the parts of each ghost's model glow once its rig has been
/// spawned: they add to what is behind them (`STATUS_BIT_GLOW`, which
/// also writes no depth), unfogged (`STATUS_BIT_NOFOG`).
///
/// Port of the status bits `MakeGhostAnt` sets
/// (original/src/Enemies/Enemy_Ant.c).
pub(super) fn make_ghosts_glow(
    mut commands: Commands,
    ghosts: Query<&Children, With<GhostGlow>>,
    parts: Query<&MeshMaterial3d<StandardMaterial>, Without<Glowing>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for children in &ghosts {
        for &child in children {
            let Ok(material) = parts.get(child) else {
                continue;
            };
            let Some(base) = materials.get(&material.0).cloned() else {
                continue;
            };
            let glow = materials.add(StandardMaterial {
                alpha_mode: AlphaMode::Add,
                fog_enabled: false,
                ..base
            });
            commands
                .entity(child)
                .insert((MeshMaterial3d(glow), Glowing));
        }
    }
}
