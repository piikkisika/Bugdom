//! Bugdom, ported to Bevy. The game binary and the development tools (such
//! as the model viewer) share these plugins.

pub mod assets;
pub mod camera;
pub mod collision;
pub mod combat;
pub mod dev;
pub mod effects;
pub mod enemies;
pub mod fences;
pub mod input;
pub mod items;
pub mod level;
pub mod liquids;
pub mod math;
pub mod objects;
pub mod pause;
pub mod physics;
pub mod player;
pub mod skeleton;
pub mod splines;
pub mod state;
pub mod terrain;

use bevy::prelude::*;

/// The game itself: states, levels, terrain, the player and the camera. Add it after
/// `DefaultPlugins` and [`assets::OriginalAssetsPlugin`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // The original seeds its generator from the clock at boot.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as u32);
        app.insert_resource(math::GameRandom::from_seed(seed));
        // Bevy takes at most 15 plugins per tuple, so they are grouped.
        // Engine: states, simulation, rendering and tools.
        app.add_plugins((
            state::StatePlugin,
            pause::PausePlugin,
            physics::PhysicsPlugin,
            collision::CollisionPlugin,
            input::InputPlugin,
            terrain::TerrainPlugin,
            objects::ObjectsPlugin,
            effects::EffectsPlugin,
            camera::CameraPlugin,
            skeleton::SkeletonPlugin,
            dev::CapturePlugin,
        ));
        // Content: the player and what the level is made of.
        app.add_plugins((
            player::PlayerPlugin,
            items::ItemsPlugin,
            fences::FencePlugin,
            splines::SplinesPlugin,
            liquids::LiquidsPlugin,
            enemies::EnemiesPlugin,
        ));
    }
}
