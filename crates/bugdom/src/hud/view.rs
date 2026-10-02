//! The HUD's camera, the display settings, and fitting the 3D view to the
//! band between the bars.

use bevy::camera::{ClearColorConfig, Viewport};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::math::URect;
use bevy::prelude::*;
use bevy::ui::UiSystems;

use super::{GameBand, HudTarget};
use crate::camera::{CameraTarget, GameCamera};

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DisplaySettings>()
        .add_systems(Startup, spawn_hud_camera)
        // After the layout, so the view follows this frame's band.
        .add_systems(PostUpdate, fit_game_view.after(UiSystems::Layout));
}

/// The camera the HUD is drawn with, over the whole window and after the
/// game camera. With split screen, each view gets one.
#[derive(Component, Debug, Default)]
pub struct HudCamera;

/// Draws after the 3D view.
const HUD_CAMERA_ORDER: isize = 1;

/// How the game is laid out on the screen. They are machine-wide
/// settings, like `gGamePrefs`, rather than any player's, so they are a
/// resource. The settings screen arrives in Phase 4; until then these are
/// the original's defaults (`InitDefaultPrefs`, original/src/System/Main.c).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySettings {
    /// Shows the bottom bar; without it the 3D view reaches the bottom of
    /// the screen (`gGamePrefs.showBottomBar`).
    pub show_bottom_bar: bool,
    /// Fits the HUD and the 3D view to 4:3 and pillarboxes them
    /// (`gGamePrefs.force4x3AspectRatio`).
    pub force_4x3: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            show_bottom_bar: true,
            force_4x3: false,
        }
    }
}

fn spawn_hud_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("HUD camera"),
        HudCamera,
        Camera2d,
        Camera {
            order: HUD_CAMERA_ORDER,
            // The game camera has drawn the band; the bars cover the rest.
            clear_color: ClearColorConfig::None,
            ..default()
        },
        // The art's colours go straight to the screen, as the game's do.
        Tonemapping::None,
    ));
}

/// The pixels of the 3D view for a band whose edges are at `min` and `max`
/// (in physical pixels, possibly fractional), within a target of
/// `target_size`, or `None` if nothing of it is on the target.
///
/// Port of the rounding in `Render_GetAdjustedViewportRect`
/// (original/src/QD3D/Renderer.c): the top-left edges round down and the
/// bottom-right ones up, so that no seam shows between the bars and the 3D
/// view. The bars, drawn after it, cover the overlap.
pub fn game_band(min: Vec2, max: Vec2, target_size: UVec2) -> Option<URect> {
    let limit = target_size.as_vec2();
    let min = min.floor().clamp(Vec2::ZERO, limit);
    let max = max.ceil().clamp(Vec2::ZERO, limit);
    let size = max - min;
    (size.x >= 1.0 && size.y >= 1.0).then(|| URect::from_corners(min.as_uvec2(), max.as_uvec2()))
}

/// Clips each game camera's view to the band between the bars of the HUD
/// of the player it follows, or lets it fill its target when there is no
/// HUD (between levels).
///
/// Port of `paneClip` (`InitArea`, original/src/System/Main.c) and of the
/// viewport set-up in `Render_GetAdjustedViewportRect` and
/// `CalcCameraMatrixInfo` (original/src/QD3D). The band comes from the HUD's
/// layout, so the bars and the 3D view are defined in one place; the
/// projection's aspect ratio follows the viewport, as `aspectRatio` follows
/// the pane. Runs every frame, but only writes the camera when the band
/// moves, such as when the window is resized or [`DisplaySettings`]
/// changes the layout.
fn fit_game_view(
    bands: Query<
        (
            &ComputedNode,
            &UiGlobalTransform,
            &HudTarget,
            &ComputedUiTargetCamera,
        ),
        With<GameBand>,
    >,
    hud_cameras: Query<&Camera, (With<super::HudCamera>, Without<GameCamera>)>,
    mut game_cameras: Query<(&mut Camera, Option<&CameraTarget>), With<GameCamera>>,
) {
    for (mut camera, target) in &mut game_cameras {
        let band = bands
            .iter()
            .find(|(.., hud_target, _)| target.is_some_and(|target| target.0 == hud_target.0));
        let viewport = band.and_then(|(node, transform, _, ui_camera)| {
            let hud_camera = hud_cameras.get(ui_camera.get()?).ok()?;
            let view = hud_camera.physical_viewport_rect()?;
            let target_size = camera.physical_target_size()?;
            // The transform is the node's centre in the HUD view.
            let center = transform.translation + view.min.as_vec2();
            let half = node.size() / 2.0;
            game_band(center - half, center + half, target_size)
        });
        let viewport = viewport.map(|rect| Viewport {
            physical_position: rect.min,
            physical_size: rect.size(),
            ..default()
        });
        let unchanged = match (&camera.viewport, &viewport) {
            (None, None) => true,
            (Some(old), Some(new)) => {
                old.physical_position == new.physical_position
                    && old.physical_size == new.physical_size
            }
            _ => false,
        };
        if !unchanged {
            camera.viewport = viewport;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The band at 640×480: the original's own screen.
    #[test]
    fn the_band_at_640_by_480_is_the_original_pane() {
        let band = game_band(
            Vec2::new(0.0, 62.0),
            Vec2::new(640.0, 420.0),
            UVec2::new(640, 480),
        );
        assert_eq!(band, Some(URect::new(0, 62, 640, 420)),);
    }

    #[test]
    fn fractional_edges_round_outward() {
        // 1920×1080: the top bar ends at 62 × 2.25 = 139.5 and the bottom
        // bar starts at 1080 − 60 × 2.25 = 945.
        let band = game_band(
            Vec2::new(0.0, 139.5),
            Vec2::new(1920.0, 945.0),
            UVec2::new(1920, 1080),
        );
        assert_eq!(band, Some(URect::new(0, 139, 1920, 945)));
        let band = game_band(
            Vec2::new(0.4, 10.2),
            Vec2::new(99.6, 50.1),
            UVec2::new(100, 100),
        );
        assert_eq!(band, Some(URect::new(0, 10, 100, 51)));
    }

    #[test]
    fn a_band_off_the_target_or_empty_is_none() {
        assert_eq!(
            game_band(
                Vec2::new(0.0, 50.0),
                Vec2::new(100.0, 50.0),
                UVec2::new(100, 100)
            ),
            None
        );
        let clamped = game_band(
            Vec2::new(-5.0, -5.0),
            Vec2::new(105.0, 105.0),
            UVec2::new(100, 100),
        );
        assert_eq!(clamped, Some(URect::new(0, 0, 100, 100)));
    }
}
