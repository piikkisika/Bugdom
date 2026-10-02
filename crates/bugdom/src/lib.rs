//! Bugdom, ported to Bevy. The game binary and the development tools (such
//! as the model viewer) share these plugins.

pub mod assets;
pub mod camera;
pub mod collision;
pub mod dev;
pub mod input;
pub mod level;
pub mod math;
pub mod physics;
pub mod player;
pub mod skeleton;
pub mod state;
pub mod terrain;

use bevy::prelude::*;

/// The game itself: states, levels, terrain, the player and the camera. Add it after
/// `DefaultPlugins` and [`assets::OriginalAssetsPlugin`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            state::StatePlugin,
            physics::PhysicsPlugin,
            input::InputPlugin,
            terrain::TerrainPlugin,
            player::PlayerPlugin,
            camera::CameraPlugin,
            skeleton::SkeletonPlugin,
            dev::CapturePlugin,
        ));
    }
}
