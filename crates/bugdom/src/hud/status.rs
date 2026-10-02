//! Health and spare lives (`ShowHealth`, `ShowLives` in original/src/Screens/Infobar.c).
//!
//! ```text
//! TopBar
//! ├─ HealthBar  black, a row: the red fill, then the yellow margin line
//! └─ LivesRow   a row of three slots, one per spare-life icon
//! ```

use bevy::prelude::*;

use super::gauge::TIMER_X;
use super::{Bar, HudSystems, HudTarget, InfobarArt, InfobarSprite, TopBar, bar_node};
use crate::combat::Health;
use crate::player::{Inventory, PLAYER_MAX_HEALTH, Player};
use crate::state::AppState;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        OnEnter(AppState::InGame),
        spawn_status.after(HudSystems::Spawn),
    )
    .add_systems(
        Update,
        (show_health, show_lives).in_set(HudSystems::Refresh),
    );
}

/// The centre of the health bar, under the ball-time gauge (`HEALTH_X`).
const HEALTH_X: f32 = TIMER_X;
/// Top of the health bar (`HEALTH_Y`).
const HEALTH_Y: f32 = 50.0;
/// The health bar's size in pixels (`HEALTH_WIDTH`, `HEALTH_HEIGHT`).
const HEALTH_WIDTH: f32 = 92.0;
const HEALTH_HEIGHT: f32 = 7.0;
/// Width of the yellow line at the end of the red fill, in pixels.
const HEALTH_MARGIN_WIDTH: i32 = 2;
/// The fill, the margin line and the empty part (`ShowHealth`).
const HEALTH_FILL_COLOR: Color = Color::srgb_u8(0xF7, 0x00, 0x18);
const HEALTH_MARGIN_COLOR: Color = Color::srgb_u8(0xFF, 0xF7, 0x00);
const HEALTH_EMPTY_COLOR: Color = Color::BLACK;

/// Where the first spare-life icon goes (`LIVES_X`, `LIVES_Y`), and how far
/// apart the icons are (`LIVES_WIDTH`), in pixels.
const LIVES_X: f32 = 11.0;
const LIVES_Y: f32 = 0.0;
const LIVES_WIDTH: f32 = 42.0;
/// The spare-life icons, each a different picture (`SPRITE_LIFE1..3`).
const LIFE_ICONS: [InfobarSprite; 3] = [
    InfobarSprite::Life1,
    InfobarSprite::Life2,
    InfobarSprite::Life3,
];

/// The health bar, with the nodes [`show_health`] sizes.
#[derive(Component, Debug)]
struct HealthBar {
    fill: Entity,
    margin: Entity,
}

/// One spare-life icon: the `n`th, from the left.
#[derive(Component, Debug)]
struct LifeIcon(usize);

/// What the health bar shows: the red fill's width and whether the yellow
/// line after it shows, in pixels of the original.
#[derive(Debug, Clone, Copy, PartialEq)]
struct HealthBarFill {
    filled: i32,
    margin: bool,
}

/// The health bar for `health`, from 0 to 1.
///
/// The numbers of `ShowHealth` (original/src/Screens/Infobar.c): the fill
/// is `⌊92 × health⌋` pixels, and the 2-pixel line after it is drawn unless
/// it would end within 2 pixels of the bar's right end.
fn health_bar(health: f32) -> HealthBarFill {
    let width = HEALTH_WIDTH as i32;
    let filled = ((HEALTH_WIDTH * health) as i32).clamp(0, width);
    HealthBarFill {
        filled,
        margin: filled + HEALTH_MARGIN_WIDTH < width - HEALTH_MARGIN_WIDTH,
    }
}

/// How many spare-life icons show with `lives` lives, the one being played
/// included (`ShowLives`: icon `i` shows if `i < gNumLives - 1`).
fn lives_shown(lives: u8) -> usize {
    usize::from(lives.saturating_sub(1)).min(LIFE_ICONS.len())
}

/// Spawns the health bar and the spare-life icons on each HUD's top bar.
fn spawn_status(
    mut commands: Commands,
    art: Res<InfobarArt>,
    bars: Query<(Entity, &HudTarget), With<TopBar>>,
) {
    for (bar, &target) in &bars {
        let part = |color: Color, width: Val| {
            (
                target,
                Node {
                    width,
                    height: Val::Percent(100.0),
                    flex_shrink: 0.0,
                    ..default()
                },
                BackgroundColor(color),
            )
        };
        let fill = commands
            .spawn((Name::new("Health fill"), part(HEALTH_FILL_COLOR, Val::ZERO)))
            .id();
        let margin = commands
            .spawn((
                Name::new("Health margin"),
                part(
                    HEALTH_MARGIN_COLOR,
                    Val::Percent(100.0 * HEALTH_MARGIN_WIDTH as f32 / HEALTH_WIDTH),
                ),
            ))
            .id();
        let health = commands
            .spawn((
                Name::new("Health"),
                HealthBar { fill, margin },
                target,
                bar_node(
                    Bar::Top,
                    HEALTH_X - HEALTH_WIDTH / 2.0,
                    HEALTH_Y,
                    HEALTH_WIDTH,
                    HEALTH_HEIGHT,
                ),
                BackgroundColor(HEALTH_EMPTY_COLOR),
            ))
            .add_children(&[fill, margin])
            .id();

        let lives = commands
            .spawn((
                Name::new("Lives"),
                target,
                bar_node(
                    Bar::Top,
                    LIVES_X,
                    LIVES_Y,
                    LIVES_WIDTH * LIFE_ICONS.len() as f32,
                    Bar::Top.height(),
                ),
            ))
            .with_children(|row| {
                for (n, &sprite) in LIFE_ICONS.iter().enumerate() {
                    // Each icon has a slot `LIVES_WIDTH` wide, as the
                    // original steps by it; the icons themselves keep their
                    // own widths.
                    row.spawn((
                        target,
                        Node {
                            width: Val::Percent(100.0 / LIFE_ICONS.len() as f32),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        children![(
                            Name::new("Spare life"),
                            LifeIcon(n),
                            target,
                            Node {
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            // The default `Auto` mode measures the image, so the
                            // width follows the art's aspect ratio.
                            ImageNode::new(art.sprite(sprite)),
                            Visibility::Hidden,
                        )],
                    ));
                }
            })
            .id();
        commands.entity(bar).add_children(&[health, lives]);
    }
}

/// Sizes the health bar's fill and shows or hides its margin line.
///
/// Port of `ShowHealth` (original/src/Screens/Infobar.c).
fn show_health(
    bars: Query<(Ref<HealthBar>, &HudTarget)>,
    players: Query<Ref<Health>, With<Player>>,
    mut parts: Query<&mut Node>,
) {
    for (bar, target) in &bars {
        let Ok(health) = players.get(target.0) else {
            continue;
        };
        if !health.is_changed() && !bar.is_added() {
            continue;
        }
        let shown = health_bar(health.0 / PLAYER_MAX_HEALTH);
        if let Ok(mut fill) = parts.get_mut(bar.fill) {
            let width = Val::Percent(100.0 * shown.filled as f32 / HEALTH_WIDTH);
            if fill.width != width {
                fill.width = width;
            }
        }
        if let Ok(mut margin) = parts.get_mut(bar.margin) {
            let display = if shown.margin {
                Display::Flex
            } else {
                Display::None
            };
            if margin.display != display {
                margin.display = display;
            }
        }
    }
}

/// Shows a spare-life icon for each life after the one being played.
///
/// Port of `ShowLives` (original/src/Screens/Infobar.c).
fn show_lives(
    mut icons: Query<(Ref<LifeIcon>, &HudTarget, &mut Visibility)>,
    players: Query<Ref<Inventory>, With<Player>>,
) {
    for (icon, target, mut visibility) in &mut icons {
        let Ok(inventory) = players.get(target.0) else {
            continue;
        };
        if !inventory.is_changed() && !icon.is_added() {
            continue;
        }
        visibility.set_if_neq(if icon.0 < lives_shown(inventory.lives) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_health_bar_fills_92_pixels() {
        assert_eq!(
            health_bar(1.0),
            HealthBarFill {
                filled: 92,
                margin: false
            }
        );
        // 92 × 0.4 = 36.8, truncated as the C conversion does.
        assert_eq!(
            health_bar(0.4),
            HealthBarFill {
                filled: 36,
                margin: true
            }
        );
        // An empty bar still has the line at its left end.
        assert_eq!(
            health_bar(0.0),
            HealthBarFill {
                filled: 0,
                margin: true
            }
        );
    }

    #[test]
    fn the_margin_line_stops_two_pixels_before_the_end() {
        // The line ends at 87 + 2 = 89 < 90; at 88 it would end at 90.
        assert!(health_bar(87.5 / 92.0).margin);
        assert!(!health_bar(88.5 / 92.0).margin);
    }

    #[test]
    fn health_out_of_range_stays_in_the_bar() {
        assert_eq!(health_bar(-0.5).filled, 0);
        assert_eq!(health_bar(1.5).filled, 92);
    }

    #[test]
    fn spare_lives_leave_out_the_one_being_played() {
        assert_eq!(lives_shown(0), 0);
        assert_eq!(lives_shown(1), 0);
        assert_eq!(lives_shown(3), 2);
        assert_eq!(lives_shown(4), 3);
        // Only three icons fit.
        assert_eq!(lives_shown(9), 3);
    }
}
