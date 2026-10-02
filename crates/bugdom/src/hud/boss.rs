//! The boss bar (`ShowBossHealth` in original/src/Screens/Infobar.c).
//!
//! ```text
//! BottomBar
//! └─ BossFrame   black, on the boss levels only
//!    └─ BossFill the boss's health, in red
//! ```

use bevy::prelude::*;

use super::{Bar, BottomBar, HudSystems, HudTarget, SCREEN_WIDTH, bar_node};
use crate::combat::{BossHealthBar, Health};
use crate::level::CurrentLevel;
use crate::state::AppState;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        OnEnter(AppState::InGame),
        spawn_boss_bar.after(HudSystems::Spawn),
    )
    .add_systems(Update, show_boss_health.in_set(HudSystems::Refresh));
}

/// The frame's size (`BOSS_WIDTH`, and the 20 rows `ShowBossHealth`
/// fills), in pixels.
const BOSS_WIDTH: f32 = 200.0;
const BOSS_HEIGHT: f32 = 20.0;
/// The frame is centred on the screen, 20 pixels down the bottom bar.
const BOSS_X: f32 = (SCREEN_WIDTH - BOSS_WIDTH) / 2.0;
const BOSS_Y_IN_BAR: f32 = 20.0;
/// The black border around the fill, in pixels.
const BOSS_BORDER: f32 = 2.0;
const BOSS_FRAME_COLOR: Color = Color::BLACK;
const BOSS_FILL_COLOR: Color = Color::srgb_u8(0xDD, 0x08, 0x06);

/// The boss bar's frame.
#[derive(Component, Debug)]
struct BossFrame;

/// The red fill in the boss bar.
#[derive(Component, Debug)]
struct BossFill;

/// The red fill's width, in pixels, for a boss with `health` left as a
/// share of its full health.
///
/// The numbers of `ShowBossHealth` (original/src/Screens/Infobar.c):
/// `w = ⌊200 × health⌋`, with health clamped at 0, and the fill is
/// `w − 4` wide, starting inside the 2-pixel border; it isn't drawn when
/// that is not positive.
fn boss_fill_width(health: f32) -> f32 {
    let w = (BOSS_WIDTH * health.max(0.0)) as i32;
    (w - 2 * BOSS_BORDER as i32).max(0) as f32
}

/// The fill's width as a share of the frame.
fn boss_fill_percent(health: f32) -> Val {
    Val::Percent(100.0 * boss_fill_width(health) / BOSS_WIDTH)
}

/// Spawns the boss bar on each HUD's bottom bar, on the boss levels. It
/// starts full, as `ShowBossHealth` draws it before the boss exists.
fn spawn_boss_bar(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    bars: Query<(Entity, &HudTarget), With<BottomBar>>,
) {
    if !level.def().is_boss_level {
        return;
    }
    let bottom = Bar::Bottom.y();
    for (bar, &target) in &bars {
        let frame = commands
            .spawn((
                Name::new("Boss health"),
                BossFrame,
                target,
                bar_node(
                    Bar::Bottom,
                    BOSS_X,
                    bottom + BOSS_Y_IN_BAR,
                    BOSS_WIDTH,
                    BOSS_HEIGHT,
                ),
                BackgroundColor(BOSS_FRAME_COLOR),
                children![(
                    Name::new("Boss health fill"),
                    BossFill,
                    target,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(100.0 * BOSS_BORDER / BOSS_WIDTH),
                        top: Val::Percent(100.0 * BOSS_BORDER / BOSS_HEIGHT),
                        width: boss_fill_percent(1.0),
                        height: Val::Percent(
                            100.0 * (BOSS_HEIGHT - 2.0 * BOSS_BORDER) / BOSS_HEIGHT
                        ),
                        ..default()
                    },
                    BackgroundColor(BOSS_FILL_COLOR),
                )],
            ))
            .id();
        commands.entity(bar).add_child(frame);
    }
}

/// Follows the health of the boss the bar shows.
///
/// Port of `ShowBossHealth` (original/src/Screens/Infobar.c), which the
/// hive, the queen bee and the king ant ask for when they are hurt. Only a
/// change on a boss that was already there counts: a boss that has just
/// spawned leaves the bar as it was until it is hurt, and so does one that
/// is gone, as the original only redraws the bar when a boss is hit.
fn show_boss_health(
    bosses: Query<(Ref<Health>, &BossHealthBar)>,
    mut fills: Query<&mut Node, With<BossFill>>,
) {
    let Some(health) = bosses
        .iter()
        .filter(|(health, _)| health.is_changed() && !health.is_added())
        .map(|(health, bar)| {
            if bar.full > 0.0 {
                health.0 / bar.full
            } else {
                0.0
            }
        })
        .last()
    else {
        return;
    };
    let width = boss_fill_percent(health);
    for mut fill in &mut fills {
        if fill.width != width {
            fill.width = width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_is_inside_the_border() {
        assert_eq!(boss_fill_width(1.0), 196.0);
        assert_eq!(boss_fill_width(0.5), 96.0);
        // 200 × 0.333 = 66.6, truncated.
        assert_eq!(boss_fill_width(0.333), 62.0);
    }

    #[test]
    fn an_empty_or_overkilled_boss_has_no_fill() {
        assert_eq!(boss_fill_width(0.0), 0.0);
        assert_eq!(boss_fill_width(0.01), 0.0);
        // The hive's health goes negative if it is shot again.
        assert_eq!(boss_fill_width(-0.3), 0.0);
    }

    fn fill_width(world: &mut World) -> Val {
        let mut fills = world.query_filtered::<&Node, With<BossFill>>();
        fills.single(world).expect("one fill").width
    }

    #[test]
    fn the_bar_follows_a_hurt_boss_only() {
        let mut world = World::new();
        // A schedule keeps the system's change ticks between runs.
        let mut schedule = Schedule::default();
        schedule.add_systems(show_boss_health);
        world.spawn((
            BossFill,
            Node {
                width: boss_fill_percent(1.0),
                ..default()
            },
        ));
        schedule.run(&mut world);

        // A freshly spawned boss doesn't change the bar, even hurt.
        let boss = world.spawn((Health(3.5), BossHealthBar { full: 7.0 })).id();
        schedule.run(&mut world);
        assert_eq!(fill_width(&mut world), boss_fill_percent(1.0));

        // A hurt boss does: the queen bee, full at 7, at half health.
        world
            .entity_mut(boss)
            .get_mut::<Health>()
            .expect("health")
            .0 = 3.5;
        schedule.run(&mut world);
        assert_eq!(fill_width(&mut world), Val::Percent(100.0 * 96.0 / 200.0));

        // One that is gone leaves it as it was.
        world.despawn(boss);
        schedule.run(&mut world);
        assert_eq!(fill_width(&mut world), Val::Percent(100.0 * 96.0 / 200.0));
    }
}
