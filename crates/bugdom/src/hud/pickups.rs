//! The ladybugs and the clovers (`ShowLadyBugs`, `ShowGoldClover`, `ShowBlueClover` in original/src/Screens/Infobar.c).
//!
//! ```text
//! TopBar
//! ├─ CloverIcon(Gold), CloverIcon(Blue)
//! ├─ LadybugIcon       the big ladybug, which changes once all are free
//! └─ FreedLadybugs     columns of two small ladybugs, one per freed one
//! ```

use bevy::prelude::*;

use super::{
    Bar, HudSystems, HudTarget, InfobarArt, InfobarSprite, SCREEN_WIDTH, TopBar, bar_node,
};
use crate::player::{Inventory, Player};
use crate::state::AppState;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        OnEnter(AppState::InGame),
        spawn_pickups.after(HudSystems::Spawn),
    )
    .add_systems(
        Update,
        (show_clovers, show_ladybugs).in_set(HudSystems::Refresh),
    );
}

/// The clovers' places (`GOLD_CLOVER_X`, …) and their sprites' sizes.
const GOLD_CLOVER_X: f32 = 141.0;
const GOLD_CLOVER_Y: f32 = 0.0;
const GOLD_CLOVER_SIZE: Vec2 = Vec2::new(55.0, 59.0);
const BLUE_CLOVER_X: f32 = 196.0;
const BLUE_CLOVER_Y: f32 = 0.0;
const BLUE_CLOVER_SIZE: Vec2 = Vec2::new(54.0, 59.0);
/// The big ladybug's place (`LADYBUG_X`, `LADYBUG_Y`) and its sprites' size.
const LADYBUG_X: f32 = 396.0;
const LADYBUG_Y: f32 = 0.0;
const LADYBUG_SIZE: Vec2 = Vec2::new(74.0, 62.0);
/// Where the freed ladybugs start, right of the big one, in pixels
/// (`ShowLadyBugs`).
const FREED_LADYBUGS_OFFSET: Vec2 = Vec2::new(80.0, 7.0);
/// The second ladybug of each column is this much lower.
const FREED_LADYBUG_ROW_STEP: f32 = 23.0;
/// Each column of two is this much right of the last.
const FREED_LADYBUG_COLUMN_STEP: f32 = 22.0;
/// How many freed ladybugs there are per column.
const FREED_LADYBUGS_PER_COLUMN: usize = 2;

/// Which clover an icon counts.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum CloverIcon {
    Gold,
    Blue,
}

impl CloverIcon {
    /// The sprites for 1, 2, 3 and 4 or more clovers.
    const fn sprites(self) -> [InfobarSprite; 4] {
        use InfobarSprite as S;
        match self {
            Self::Gold => [
                S::GoldClover1,
                S::GoldClover2,
                S::GoldClover3,
                S::GoldClover4,
            ],
            Self::Blue => [
                S::BlueClover1,
                S::BlueClover2,
                S::BlueClover3,
                S::BlueClover4,
            ],
        }
    }

    fn count(self, inventory: &Inventory) -> u16 {
        match self {
            Self::Gold => inventory.gold_clovers,
            Self::Blue => inventory.blue_clovers,
        }
    }
}

/// The big ladybug.
#[derive(Component, Debug)]
struct LadybugIcon;

/// The node holding a small ladybug for each one freed. It wraps them into
/// columns of two and clips them at the screen's right edge.
#[derive(Component, Debug)]
struct FreedLadybugs;

/// Which of the four pictures shows `count` clovers, or `None` to show
/// none (`ShowGoldClover`, `ShowBlueClover`: 1 to 3 have their own, and
/// any more show the fourth).
fn clover_sprite(count: u16) -> Option<usize> {
    (count > 0).then(|| usize::from(count.min(4)) - 1)
}

/// Where the `i`th freed ladybug goes on the 640×480 screen (the loop in
/// `ShowLadyBugs`): in columns of two, the first at the top.
///
/// The ladybugs are laid out by a wrapping flex column instead; this gives
/// that layout its origin and steps.
fn ladybug_position(i: usize) -> Vec2 {
    let column = (i / FREED_LADYBUGS_PER_COLUMN) as f32;
    let row = (i % FREED_LADYBUGS_PER_COLUMN) as f32;
    Vec2::new(LADYBUG_X, LADYBUG_Y)
        + FREED_LADYBUGS_OFFSET
        + Vec2::new(
            FREED_LADYBUG_COLUMN_STEP * column,
            FREED_LADYBUG_ROW_STEP * row,
        )
}

/// Spawns the clovers and the ladybugs on each HUD's top bar.
fn spawn_pickups(
    mut commands: Commands,
    art: Res<InfobarArt>,
    bars: Query<(Entity, &HudTarget), With<TopBar>>,
) {
    let freed = ladybug_position(0);
    for (bar, &target) in &bars {
        let clover = |icon: CloverIcon, x: f32, y: f32, size: Vec2| {
            (
                Name::new("Clover"),
                icon,
                target,
                bar_node(Bar::Top, x, y, size.x, size.y),
                ImageNode::new(art.sprite(icon.sprites()[0])).with_mode(NodeImageMode::Stretch),
                Visibility::Hidden,
            )
        };
        let gold = commands
            .spawn(clover(
                CloverIcon::Gold,
                GOLD_CLOVER_X,
                GOLD_CLOVER_Y,
                GOLD_CLOVER_SIZE,
            ))
            .id();
        let blue = commands
            .spawn(clover(
                CloverIcon::Blue,
                BLUE_CLOVER_X,
                BLUE_CLOVER_Y,
                BLUE_CLOVER_SIZE,
            ))
            .id();
        let ladybug = commands
            .spawn((
                Name::new("Ladybug"),
                LadybugIcon,
                target,
                bar_node(
                    Bar::Top,
                    LADYBUG_X,
                    LADYBUG_Y,
                    LADYBUG_SIZE.x,
                    LADYBUG_SIZE.y,
                ),
                ImageNode::new(art.sprite(InfobarSprite::Ladybug))
                    .with_mode(NodeImageMode::Stretch),
            ))
            .id();
        let freed_ladybugs = commands
            .spawn((
                Name::new("Freed ladybugs"),
                FreedLadybugs,
                target,
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_wrap: FlexWrap::Wrap,
                    // Columns stay their own width rather than sharing out
                    // the free space.
                    align_content: AlignContent::FlexStart,
                    align_items: AlignItems::FlexStart,
                    // The original draws past the screen's edge, where
                    // nothing shows.
                    overflow: Overflow::clip(),
                    ..bar_node(
                        Bar::Top,
                        freed.x,
                        freed.y,
                        SCREEN_WIDTH - freed.x,
                        FREED_LADYBUG_ROW_STEP * FREED_LADYBUGS_PER_COLUMN as f32,
                    )
                },
            ))
            .id();
        commands
            .entity(bar)
            .add_children(&[gold, blue, ladybug, freed_ladybugs]);
    }
}

/// Shows each clover's picture for how many there are, or none.
///
/// Port of `ShowGoldClover` and `ShowBlueClover`
/// (original/src/Screens/Infobar.c).
fn show_clovers(
    art: Res<InfobarArt>,
    mut icons: Query<(Ref<CloverIcon>, &HudTarget, &mut ImageNode, &mut Visibility)>,
    players: Query<Ref<Inventory>, With<Player>>,
) {
    for (icon, target, mut image, mut visibility) in &mut icons {
        let Ok(inventory) = players.get(target.0) else {
            continue;
        };
        if !inventory.is_changed() && !icon.is_added() {
            continue;
        }
        match clover_sprite(icon.count(&inventory)) {
            Some(index) => {
                let sprite = art.sprite(icon.sprites()[index]);
                if image.image != sprite {
                    image.image = sprite;
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// Shows the big ladybug, which changes once every ladybug in the area is
/// free, and a small one for each freed ladybug.
///
/// Port of `ShowLadyBugs` (original/src/Screens/Infobar.c).
fn show_ladybugs(
    mut commands: Commands,
    art: Res<InfobarArt>,
    mut icons: Query<(Ref<LadybugIcon>, &HudTarget, &mut ImageNode)>,
    rows: Query<(Entity, Ref<FreedLadybugs>, &HudTarget, Option<&Children>)>,
    players: Query<Ref<Inventory>, With<Player>>,
) {
    for (icon, target, mut image) in &mut icons {
        let Ok(inventory) = players.get(target.0) else {
            continue;
        };
        if !inventory.is_changed() && !icon.is_added() {
            continue;
        }
        let sprite = art.sprite(if inventory.has_all_ladybugs() {
            InfobarSprite::LadybugAll
        } else {
            InfobarSprite::Ladybug
        });
        if image.image != sprite {
            image.image = sprite;
        }
    }

    for (row, freed, &target, children) in &rows {
        let Ok(inventory) = players.get(target.0) else {
            continue;
        };
        if !inventory.is_changed() && !freed.is_added() {
            continue;
        }
        let shown = children.map_or(0, |children| children.len());
        let wanted = usize::from(inventory.ladybugs);
        if let Some(children) = children {
            for child in children.iter().skip(wanted) {
                commands.entity(child).despawn();
            }
        }
        if wanted > shown {
            let width = freed_ladybugs_width();
            commands.entity(row).with_children(|row| {
                for _ in shown..wanted {
                    row.spawn((
                        Name::new("Freed ladybug"),
                        target,
                        Node {
                            width: Val::Percent(100.0 * FREED_LADYBUG_COLUMN_STEP / width),
                            height: Val::Percent(100.0 / FREED_LADYBUGS_PER_COLUMN as f32),
                            ..default()
                        },
                        children![(
                            target,
                            Node {
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            // The default `Auto` mode measures the image, so the
                            // width follows the art's aspect ratio.
                            ImageNode::new(art.sprite(InfobarSprite::LadybugSmall)),
                        )],
                    ));
                }
            });
        }
    }
}

/// The width of the freed ladybugs' node, in pixels: up to the screen's
/// right edge.
fn freed_ladybugs_width() -> f32 {
    SCREEN_WIDTH - ladybug_position(0).x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clovers_show_one_to_four() {
        assert_eq!(clover_sprite(0), None);
        assert_eq!(clover_sprite(1), Some(0));
        assert_eq!(clover_sprite(3), Some(2));
        assert_eq!(clover_sprite(4), Some(3));
        assert_eq!(clover_sprite(12), Some(3));
    }

    #[test]
    fn freed_ladybugs_go_in_columns_of_two() {
        // `x = LADYBUG_X + 80`, `y = LADYBUG_Y + 7` or `+ 30`, and 22 right
        // after each odd one.
        assert_eq!(ladybug_position(0), Vec2::new(476.0, 7.0));
        assert_eq!(ladybug_position(1), Vec2::new(476.0, 30.0));
        assert_eq!(ladybug_position(2), Vec2::new(498.0, 7.0));
        assert_eq!(ladybug_position(5), Vec2::new(520.0, 30.0));
    }

    #[test]
    fn the_flex_layout_matches_the_original_steps() {
        // The flex column's slots are as tall as a row step and as wide as
        // a column step, so the slots land where the original draws.
        let small = bugdom_formats::tga::open(
            bugdom_formats::original_data_dir().join("Images/Infobar/141.tga"),
        )
        .expect("141.tga");
        assert_eq!(small.height as f32, FREED_LADYBUG_ROW_STEP);
        assert!(small.width as f32 <= FREED_LADYBUG_COLUMN_STEP);
        // The first 14 fit before the screen's right edge.
        let fits = (0..)
            .take_while(|&i| ladybug_position(i).x + small.width as f32 <= SCREEN_WIDTH)
            .count();
        assert_eq!(fits, 14);
        assert_eq!(freed_ladybugs_width(), 164.0);
    }
}
