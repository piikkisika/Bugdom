//! The in-game HUD: the infobar above and below the 3D view.
//!
//! Port of the drawing half of original/src/Screens/Infobar.c, written as
//! Bevy UI (see docs/design/phase4-hud.md). Each locally controlled player
//! gets a HUD, whose root fills the [`HudCamera`]'s view as a column:
//!
//! ```text
//! HudRoot (column, 100% × 100%)
//! ├─ TopBar     100% × 62/480, the top bar's art
//! ├─ GameBand   the rest: the game camera's viewport follows it
//! └─ BottomBar  100% × 60/480, the bottom bar's art
//! ```
//!
//! The bars stretch with the window, as the source port's 640×480 overlay
//! does. Elements are children of their bar, placed with [`bar_node`] in
//! percentages of the bar, so they stay on their spot of the art at any
//! size. Each element module adds a system in `OnEnter(AppState::InGame)`,
//! after [`HudSystems::Spawn`], that spawns its nodes under the bar of each
//! HUD and copies the bar's [`HudTarget`] onto them; its refresh systems
//! run in [`HudSystems::Refresh`].
//!
//! The original composites everything into one texture and redraws only
//! what `gInfobarUpdateBits` says has changed. Here each element is its
//! own node, which Bevy redraws when it changes.

mod art;
mod boss;
mod gauge;
mod pickups;
mod status;
mod view;

use bevy::prelude::*;
use bevy::ui::LayoutConfig;

pub use art::{InfobarArt, InfobarSprite};
pub use view::{DisplaySettings, HudCamera, game_band};

use crate::input::LocalControls;
use crate::player::{Player, PlayerSystems};
use crate::state::AppState;

/// The layout's width and height in the original's units: everything in
/// the infobar is placed on a 640×480 screen.
pub const SCREEN_WIDTH: f32 = 640.0;
pub const SCREEN_HEIGHT: f32 = 480.0;
/// Height of the top bar, which is also where the 3D view starts
/// (`paneClip.top` in `InitArea`, original/src/System/Main.c).
pub const TOP_BAR_HEIGHT: f32 = 62.0;
/// Height of the bottom bar (`paneClip.bottom`).
pub const BOTTOM_BAR_HEIGHT: f32 = 60.0;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            HudSystems::Refresh.run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            OnEnter(AppState::InGame),
            spawn_hud
                .in_set(HudSystems::Spawn)
                .after(PlayerSystems::Spawn),
        )
        .add_plugins((
            art::plugin,
            view::plugin,
            status::plugin,
            gauge::plugin,
            pickups::plugin,
            boss::plugin,
        ));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum HudSystems {
    /// Spawns each player's HUD when an area starts (`InitInfobar`).
    /// Element modules spawn their nodes after it.
    Spawn,
    /// Brings the elements up to date with what they show
    /// (`UpdateInfobar`), each frame in `Update` while in a level.
    Refresh,
}

/// The player a HUD, or one of its nodes, shows. Like
/// [`CameraTarget`](crate::camera::CameraTarget), it lets each of several
/// players have a HUD of their own.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudTarget(pub Entity);

/// The root node of a HUD.
#[derive(Component, Debug, Default)]
pub struct HudRoot;

/// The top bar, parent of the elements drawn on it.
#[derive(Component, Debug, Default)]
pub struct TopBar;

/// The bottom bar, parent of the boss bar.
#[derive(Component, Debug, Default)]
pub struct BottomBar;

/// The space between the bars, where the 3D view goes. It draws nothing:
/// [`view`] fits the game camera's viewport to it.
#[derive(Component, Debug, Default)]
pub struct GameBand;

/// One of the two bars, for converting the original's screen coordinates
/// into a bar's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    Top,
    Bottom,
}

impl Bar {
    /// Where the bar starts on the 640×480 screen.
    pub const fn y(self) -> f32 {
        match self {
            Self::Top => 0.0,
            Self::Bottom => SCREEN_HEIGHT - BOTTOM_BAR_HEIGHT,
        }
    }

    /// The bar's height on the 640×480 screen.
    pub const fn height(self) -> f32 {
        match self {
            Self::Top => TOP_BAR_HEIGHT,
            Self::Bottom => BOTTOM_BAR_HEIGHT,
        }
    }

    /// A horizontal length in the original's units, as a share of the
    /// bar's width (which is the screen's).
    pub fn width_percent(self, width: f32) -> Val {
        Val::Percent(100.0 * width / SCREEN_WIDTH)
    }

    /// A vertical length in the original's units, as a share of the bar's
    /// height.
    pub fn height_percent(self, height: f32) -> Val {
        Val::Percent(100.0 * height / self.height())
    }
}

/// A node at `(x, y)` with size `(width, height)` on the original's
/// 640×480 screen, placed absolutely in `bar` in percentages of the bar's
/// size, so that it stays on the same spot of the bar's art at any window
/// size.
pub fn bar_node(bar: Bar, x: f32, y: f32, width: f32, height: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: bar.width_percent(x),
        top: bar.height_percent(y - bar.y()),
        width: bar.width_percent(width),
        height: bar.height_percent(height),
        ..default()
    }
}

/// Spawns a HUD for each locally controlled player: the bars with their
/// art and the band between them.
///
/// Port of `InitInfobar` (original/src/Screens/Infobar.c). Everything
/// carries `DespawnOnExit(AppState::InGame)`, so the HUD is rebuilt for
/// each area as `InitInfobar` rebuilds the infobar texture.
fn spawn_hud(
    mut commands: Commands,
    art: Res<InfobarArt>,
    cameras: Query<Entity, With<HudCamera>>,
    players: Query<Entity, (With<Player>, With<LocalControls>)>,
) {
    // One view for now; split screen gives each player a camera of its own.
    let Some(camera) = cameras.iter().min() else {
        warn!("No HUD camera to show the HUD in");
        return;
    };
    for player in &players {
        let target = HudTarget(player);
        commands.spawn((
            Name::new("HUD"),
            HudRoot,
            target,
            UiTargetCamera(camera),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            DespawnOnExit(AppState::InGame),
            children![
                (
                    Name::new("Top bar"),
                    TopBar,
                    bar_bundle(Bar::Top, art.sprite(InfobarSprite::InfobarTop), target)
                ),
                (
                    Name::new("Game band"),
                    GameBand,
                    target,
                    Node {
                        width: Val::Percent(100.0),
                        flex_grow: 1.0,
                        ..default()
                    },
                    // Unrounded, so that the viewport can round its edges
                    // outward as the original does.
                    LayoutConfig {
                        use_rounding: false
                    },
                ),
                (
                    Name::new("Bottom bar"),
                    BottomBar,
                    bar_bundle(
                        Bar::Bottom,
                        art.sprite(InfobarSprite::InfobarBottom),
                        target
                    )
                ),
            ],
        ));
    }
}

/// A bar spanning the HUD's width, with its share of the height and its
/// art stretched over it.
fn bar_bundle(bar: Bar, image: Handle<Image>, target: HudTarget) -> impl Bundle {
    (
        target,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0 * bar.height() / SCREEN_HEIGHT),
            flex_shrink: 0.0,
            ..default()
        },
        ImageNode::new(image).with_mode(NodeImageMode::Stretch),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percent(val: Val) -> f32 {
        match val {
            Val::Percent(p) => p,
            other => panic!("{other:?} is not a percentage"),
        }
    }

    #[test]
    fn bar_nodes_are_placed_in_percentages_of_their_bar() {
        // The ball icon (`BALL_X`, `BALL_Y`) on the top bar.
        let node = bar_node(Bar::Top, 307.0, 17.0, 27.0, 26.0);
        assert_eq!(node.position_type, PositionType::Absolute);
        assert!((percent(node.left) - 100.0 * 307.0 / 640.0).abs() < 1e-4);
        assert!((percent(node.top) - 100.0 * 17.0 / 62.0).abs() < 1e-4);
        assert!((percent(node.width) - 100.0 * 27.0 / 640.0).abs() < 1e-4);
        assert!((percent(node.height) - 100.0 * 26.0 / 62.0).abs() < 1e-4);
    }

    #[test]
    fn bottom_bar_nodes_are_relative_to_the_bottom_bar() {
        // The boss bar's frame (`ShowBossHealth`).
        let node = bar_node(Bar::Bottom, 220.0, 440.0, 200.0, 20.0);
        assert!((percent(node.top) - 100.0 * 20.0 / 60.0).abs() < 1e-4);
        assert!((percent(node.height) - 100.0 * 20.0 / 60.0).abs() < 1e-4);
        assert!((percent(node.left) - 100.0 * 220.0 / 640.0).abs() < 1e-4);
    }
}
