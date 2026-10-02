//! Bugdom, ported to Bevy.

use bevy::prelude::*;
use bugdom::assets::{OriginalAssetsPlugin, OriginalDataSourcePlugin};

fn main() -> AppExit {
    App::new()
        .add_plugins(OriginalDataSourcePlugin)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bugdom".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(OriginalAssetsPlugin)
        .add_systems(Startup, spawn_camera)
        .run()
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn(Camera3d::default());
}
