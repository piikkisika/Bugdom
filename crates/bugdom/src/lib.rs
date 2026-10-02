//! Bugdom, ported to Bevy. The game binary and the development tools (such
//! as the model viewer) share these plugins.

pub mod assets;
pub mod camera;
pub mod dev;
pub mod level;
pub mod skeleton;
pub mod state;
pub mod terrain;

use bevy::prelude::*;

/// The game itself: states, levels, terrain and camera. Add it after
/// `DefaultPlugins` and [`assets::OriginalAssetsPlugin`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            state::StatePlugin,
            terrain::TerrainPlugin,
            camera::CameraPlugin,
            skeleton::SkeletonPlugin,
            dev::CapturePlugin,
        ));
    }
}
