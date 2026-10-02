//! Bugdom, ported to Bevy.

use bevy::prelude::*;
use bugdom::GamePlugin;
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
        .add_plugins((OriginalAssetsPlugin, GamePlugin))
        .run()
}
