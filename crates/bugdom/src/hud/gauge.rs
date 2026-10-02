//! The ball-time gauge, the hands and the ball icon (`DrawNitroGauge`, `ShowBallTimerAndInventory` in original/src/Screens/Infobar.c).
//!
//! ```text
//! TopBar
//! ├─ BallGauge   its own image, redrawn on the CPU from NitroGauge.tga
//! ├─ HandsRow    two halves meeting at TIMER_X, one hand in each
//! └─ BallIcon    over the bug's head, in ball form
//! ```
//!
//! They are spawned in this order, so the hands and the ball icon draw
//! over the gauge, as `ShowBallTimerAndInventory` draws them after it.

use bevy::image::ImageAddressMode;
use bevy::prelude::*;

use super::{Bar, HudSystems, HudTarget, InfobarArt, InfobarSprite, TopBar, bar_node};
use crate::assets::image::{bleed_edges, rgba8_image};
use crate::player::{BallTime, DoorKey, HandItem, Inventory, Player, PlayerForm};
use crate::state::AppState;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        OnEnter(AppState::InGame),
        spawn_gauge.after(HudSystems::Spawn),
    )
    .add_systems(
        Update,
        (draw_ball_gauge, show_hands, show_ball_icon).in_set(HudSystems::Refresh),
    );
}

/// The centre of the gauge, where the hands meet (`TIMER_X`).
pub(super) const TIMER_X: f32 = 320.0;
/// Top of the gauge (`TIMER_Y`).
const TIMER_Y: f32 = 6.0;
/// Where the hands meet and their top (`HAND_X`, `HAND_Y`).
const HAND_X: f32 = TIMER_X;
const HAND_Y: f32 = 1.0;
/// Height of the hand sprites (142 to 155), in pixels; their widths vary.
const HAND_HEIGHT: f32 = 47.0;
/// The ball icon's place (`BALL_X`, `BALL_Y`) and its sprite's size.
const BALL_X: f32 = TIMER_X - 13.0;
const BALL_Y: f32 = 17.0;
const BALL_SIZE: Vec2 = Vec2::new(27.0, 26.0);

/// The gauge's sweep when the ball time is full, in degrees.
const GAUGE_FULL_DEGREES: f32 = 180.0;
/// How far the sweep must move before a timer change redraws the gauge,
/// in degrees (`gOldTimerN` in `ShowBallTimerAndInventory`).
const GAUGE_REDRAW_DEGREES: i32 = 2;
/// Width of the yellow margin line past the sweep, in degrees, and the
/// sweeps between which it is drawn (`wantMargin` in `DrawNitroGauge`).
const GAUGE_MARGIN_DEGREES: i32 = 3;
const GAUGE_MARGIN_MIN: i32 = 2;
const GAUGE_MARGIN_MAX: i32 = 178;
/// The gauge's colours (`DrawNitroGauge`).
const GAUGE_FILL: [u8; 4] = [0x00, 0xBD, 0x29, 0xFF];
const GAUGE_MARGIN: [u8; 4] = [0xFF, 0xF7, 0x00, 0xFF];
const GAUGE_EMPTY: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
/// Outside the gauge: the bar's art shows through.
const TRANSPARENT: [u8; 4] = [0; 4];

/// The ball-time gauge of one HUD. Its [`ImageNode`] holds its own image,
/// which [`draw_ball_gauge`] rewrites.
#[derive(Component, Debug)]
struct BallGauge {
    /// `NitroGauge.tga`, one value per pixel: 0 outside the gauge, else
    /// 1 + the pixel's angle in degrees.
    template: Vec<u8>,
    width: usize,
    height: usize,
    /// The sweep last drawn (`gOldTimerN`), or `None` before the first.
    drawn: Option<i32>,
}

/// The node holding both hands, hidden in ball form.
#[derive(Component, Debug)]
struct HandsRow;

/// One of the bug's hands.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum HandIcon {
    Left,
    Right,
}

/// The ball icon, shown in ball form.
#[derive(Component, Debug)]
struct BallIcon;

/// The gauge's sweep for `ball_time`, from 0 to 1, in whole degrees
/// (`n = 180.0f * gBallTimer` in `ShowBallTimerAndInventory`, after
/// clamping the timer at 0).
fn gauge_arc(ball_time: f32) -> i32 {
    (GAUGE_FULL_DEGREES * ball_time.max(0.0)) as i32
}

/// Whether the gauge needs drawing at `arc` degrees when `drawn` was
/// drawn last: only once the sweep has moved by 2° or more
/// (`ShowBallTimerAndInventory` with `ballTimerOnly`).
fn gauge_moved(drawn: Option<i32>, arc: i32) -> bool {
    drawn.is_none_or(|drawn| (arc - drawn).abs() >= GAUGE_REDRAW_DEGREES)
}

/// The colour of a gauge pixel whose template value is `value` with the
/// gauge swept to `arc` degrees, or `None` outside the gauge.
///
/// Port of the pixel rule in `DrawNitroGauge`
/// (original/src/Screens/Infobar.c): green up to the sweep, then a yellow
/// line 3° wide unless the gauge is nearly empty or full, then black.
fn gauge_color(value: u8, arc: i32) -> Option<[u8; 4]> {
    let angle = i32::from(value.checked_sub(1)?);
    let want_margin = arc > GAUGE_MARGIN_MIN && arc < GAUGE_MARGIN_MAX;
    Some(if angle <= arc {
        GAUGE_FILL
    } else if want_margin && angle <= arc + GAUGE_MARGIN_DEGREES {
        GAUGE_MARGIN
    } else {
        GAUGE_EMPTY
    })
}

/// Draws the gauge swept to `arc` degrees into `rgba`, from its template.
///
/// Port of `DrawNitroGauge` (original/src/Screens/Infobar.c). Pixels
/// outside the gauge are transparent, with their colour bled from the
/// gauge's so that filtering doesn't darken its edges.
fn draw_gauge(template: &[u8], width: usize, height: usize, arc: i32, rgba: &mut [u8]) {
    for (&value, pixel) in template.iter().zip(rgba.chunks_exact_mut(4)) {
        pixel.copy_from_slice(&gauge_color(value, arc).unwrap_or(TRANSPARENT));
    }
    bleed_edges(rgba, width, height);
}

/// The sprite of what a hand holds (`gLeftArmType`, `gRightArmType`).
fn hand_sprite(item: HandItem, hand: HandIcon) -> InfobarSprite {
    use InfobarSprite as S;
    let (left, right) = match item {
        HandItem::Empty => (S::EmptyHandL, S::EmptyHandR),
        HandItem::Money => (S::MoneyL, S::MoneyR),
        HandItem::Key(DoorKey::Green) => (S::GreenKeyL, S::GreenKeyR),
        HandItem::Key(DoorKey::Blue) => (S::BlueKeyL, S::BlueKeyR),
        HandItem::Key(DoorKey::Red) => (S::RedKeyL, S::RedKeyR),
        HandItem::Key(DoorKey::Orange) => (S::OrangeKeyL, S::OrangeKeyR),
        HandItem::Key(DoorKey::Purple) => (S::PurpleKeyL, S::PurpleKeyR),
    };
    match hand {
        HandIcon::Left => left,
        HandIcon::Right => right,
    }
}

/// Spawns the gauge, the hands and the ball icon on each HUD's top bar.
///
/// The gauge's image is made here from the template, as
/// `LoadNitroGaugeTemplate` places the gauge from the template's size.
fn spawn_gauge(
    mut commands: Commands,
    art: Res<InfobarArt>,
    mut images: ResMut<Assets<Image>>,
    bars: Query<(Entity, &HudTarget), With<TopBar>>,
) {
    let Some(template) = images.get(&art.gauge_template) else {
        warn!("The ball-time gauge's template has not loaded");
        return;
    };
    let (width, height) = (template.width() as usize, template.height() as usize);
    // The loader copies the grey value into red, green and blue.
    let template: Vec<u8> = template
        .data
        .as_deref()
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|pixel| pixel[0])
        .collect();
    if template.len() != width * height {
        warn!("The ball-time gauge's template has no pixels");
        return;
    }
    let size = Vec2::new(width as f32, height as f32);

    for (bar, &target) in &bars {
        let image = images.add(rgba8_image(
            width as u32,
            height as u32,
            vec![0; width * height * 4],
            ImageAddressMode::ClampToEdge,
            ImageAddressMode::ClampToEdge,
        ));
        let gauge = commands
            .spawn((
                Name::new("Ball-time gauge"),
                BallGauge {
                    template: template.clone(),
                    width,
                    height,
                    drawn: None,
                },
                target,
                bar_node(Bar::Top, TIMER_X - size.x / 2.0, TIMER_Y, size.x, size.y),
                ImageNode::new(image).with_mode(NodeImageMode::Stretch),
            ))
            .id();

        // The left hand ends at `HAND_X` and the right one starts there,
        // whatever their widths: each sits at the inner end of its half.
        let half = |hand: HandIcon, width: f32, justify: JustifyContent| {
            (
                target,
                Node {
                    width: Bar::Top.width_percent(width),
                    height: Val::Percent(100.0),
                    justify_content: justify,
                    ..default()
                },
                children![(
                    Name::new("Hand"),
                    hand,
                    target,
                    Node {
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    // The default `Auto` mode measures the image, so the
                    // width follows the art's aspect ratio.
                    ImageNode::new(art.sprite(hand_sprite(HandItem::Empty, hand))),
                )],
            )
        };
        let hands = commands
            .spawn((
                Name::new("Hands"),
                HandsRow,
                target,
                bar_node(Bar::Top, 0.0, HAND_Y, super::SCREEN_WIDTH, HAND_HEIGHT),
                children![
                    half(HandIcon::Left, HAND_X, JustifyContent::FlexEnd),
                    half(
                        HandIcon::Right,
                        super::SCREEN_WIDTH - HAND_X,
                        JustifyContent::FlexStart
                    ),
                ],
            ))
            .id();

        let ball = commands
            .spawn((
                Name::new("Ball icon"),
                BallIcon,
                target,
                bar_node(Bar::Top, BALL_X, BALL_Y, BALL_SIZE.x, BALL_SIZE.y),
                ImageNode::new(art.sprite(InfobarSprite::Ball)).with_mode(NodeImageMode::Stretch),
                Visibility::Hidden,
            ))
            .id();

        commands.entity(bar).add_children(&[gauge, hands, ball]);
    }
}

/// Redraws the gauge once the ball time has moved it by 2° or more, or
/// when the player changes form.
///
/// Port of `DrawNitroGauge` and of when `ShowBallTimerAndInventory`
/// (original/src/Screens/Infobar.c) calls it: a timer change redraws it
/// only past the threshold, and a form change (`UPDATE_HANDS`) always.
fn draw_ball_gauge(
    mut gauges: Query<(&mut BallGauge, &HudTarget, &ImageNode)>,
    players: Query<(&BallTime, Ref<PlayerForm>), With<Player>>,
    mut images: ResMut<Assets<Image>>,
) {
    for (mut gauge, target, image) in &mut gauges {
        let Ok((ball_time, form)) = players.get(target.0) else {
            continue;
        };
        let arc = gauge_arc(ball_time.0);
        let redraw =
            gauge_moved(gauge.drawn, arc) || (form.is_changed() && gauge.drawn != Some(arc));
        if !redraw {
            continue;
        }
        let Some(mut image) = images.get_mut(&image.image) else {
            continue;
        };
        let Some(rgba) = image.data.as_mut() else {
            continue;
        };
        draw_gauge(&gauge.template, gauge.width, gauge.height, arc, rgba);
        gauge.drawn = Some(arc);
    }
}

/// Shows what each hand holds.
///
/// Port of the hands in `ShowBallTimerAndInventory`
/// (original/src/Screens/Infobar.c). The original skips the redraw for a
/// key or coin picked up in ball form and catches up when the bug comes
/// back; the hands are hidden in ball form, so drawing them from the
/// inventory on every change looks the same.
fn show_hands(
    art: Res<InfobarArt>,
    mut hands: Query<(Ref<HandIcon>, &HudTarget, &mut ImageNode)>,
    players: Query<Ref<Inventory>, With<Player>>,
) {
    for (hand, target, mut image) in &mut hands {
        let Ok(inventory) = players.get(target.0) else {
            continue;
        };
        if !inventory.is_changed() && !hand.is_added() {
            continue;
        }
        let item = match *hand {
            HandIcon::Left => inventory.hands[0],
            HandIcon::Right => inventory.hands[1],
        };
        let sprite = art.sprite(hand_sprite(item, *hand));
        if image.image != sprite {
            image.image = sprite;
        }
    }
}

/// Shows the hands in bug form and the ball icon in ball form.
///
/// Port of the form switch in `ShowBallTimerAndInventory`
/// (original/src/Screens/Infobar.c): the ball erases the arms and covers
/// the bug's head, and the bug erases the ball.
fn show_ball_icon(
    mut hands: Query<(Ref<HandsRow>, &HudTarget, &mut Visibility), Without<BallIcon>>,
    mut balls: Query<(Ref<BallIcon>, &HudTarget, &mut Visibility), Without<HandsRow>>,
    players: Query<Ref<PlayerForm>, With<Player>>,
) {
    let shown = |is_shown: bool| {
        if is_shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    for (row, target, mut visibility) in &mut hands {
        if let Ok(form) = players.get(target.0)
            && (form.is_changed() || row.is_added())
        {
            visibility.set_if_neq(shown(*form == PlayerForm::Bug));
        }
    }
    for (icon, target, mut visibility) in &mut balls {
        if let Ok(form) = players.get(target.0)
            && (form.is_changed() || icon.is_added())
        {
            visibility.set_if_neq(shown(*form == PlayerForm::Ball));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_is_whole_degrees_of_the_ball_time() {
        assert_eq!(gauge_arc(1.0), 180);
        assert_eq!(gauge_arc(0.5), 90);
        assert_eq!(gauge_arc(0.999), 179);
        assert_eq!(gauge_arc(-0.2), 0);
    }

    #[test]
    fn the_gauge_redraws_after_two_degrees() {
        assert!(gauge_moved(None, 180));
        assert!(!gauge_moved(Some(180), 179));
        assert!(gauge_moved(Some(180), 178));
        assert!(gauge_moved(Some(90), 92));
    }

    #[test]
    fn gauge_pixels_are_green_then_yellow_then_black() {
        assert_eq!(gauge_color(0, 90), None);
        // Template values are 1 + the angle.
        assert_eq!(gauge_color(91, 90), Some(GAUGE_FILL));
        assert_eq!(gauge_color(92, 90), Some(GAUGE_MARGIN));
        assert_eq!(gauge_color(94, 90), Some(GAUGE_MARGIN));
        assert_eq!(gauge_color(95, 90), Some(GAUGE_EMPTY));
    }

    #[test]
    fn the_margin_line_is_left_out_near_the_ends() {
        assert_eq!(gauge_color(4, 2), Some(GAUGE_EMPTY));
        assert_eq!(gauge_color(5, 3), Some(GAUGE_MARGIN));
        assert_eq!(gauge_color(180, 177), Some(GAUGE_MARGIN));
        assert_eq!(gauge_color(180, 178), Some(GAUGE_EMPTY));
        assert_eq!(gauge_color(181, 180), Some(GAUGE_FILL));
    }

    #[test]
    fn hands_show_what_they_hold() {
        assert_eq!(
            hand_sprite(HandItem::Empty, HandIcon::Left),
            InfobarSprite::EmptyHandL
        );
        assert_eq!(
            hand_sprite(HandItem::Money, HandIcon::Right),
            InfobarSprite::MoneyR
        );
        // `SPRITE_GREENKEY_L + keyID * 2`, plus one for the right hand.
        for (n, key) in DoorKey::ALL.into_iter().enumerate() {
            let first = InfobarSprite::GreenKeyL as usize;
            assert_eq!(
                hand_sprite(HandItem::Key(key), HandIcon::Left) as usize,
                first + 2 * n
            );
            assert_eq!(
                hand_sprite(HandItem::Key(key), HandIcon::Right) as usize,
                first + 2 * n + 1
            );
        }
    }

    /// Draws the gauge from the real template, half full.
    #[test]
    fn the_real_gauge_draws_half_full() {
        let path = bugdom_formats::original_data_dir().join("Images/Infobar/NitroGauge.tga");
        let tga = bugdom_formats::tga::open(path).expect("NitroGauge.tga");
        let (width, height) = (tga.width as usize, tga.height as usize);
        let template: Vec<u8> = tga.pixels.chunks_exact(4).map(|p| p[0]).collect();
        let mut rgba = vec![0; width * height * 4];
        draw_gauge(&template, width, height, gauge_arc(0.5), &mut rgba);

        let pixel = |x: usize, y: usize| {
            let i = (y * width + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // The bottom-left end is the start of the sweep, the bottom-right
        // end its end, and the middle of the bar is outside the gauge.
        assert_eq!(pixel(2, height - 1), GAUGE_FILL);
        assert_eq!(pixel(width - 3, height - 1), GAUGE_EMPTY);
        assert_eq!(pixel(width / 2, height - 1)[3], 0);
        let count = |color: [u8; 4]| {
            (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(x, y) == color)
                .count()
        };
        assert!(count(GAUGE_FILL) > 0);
        assert!(count(GAUGE_MARGIN) > 0);
        assert!(count(GAUGE_EMPTY) > 0);
        // Half the arc is green: about as many pixels as are black or yellow.
        let filled = count(GAUGE_FILL) as f32;
        let rest = (count(GAUGE_MARGIN) + count(GAUGE_EMPTY)) as f32;
        assert!((filled / rest - 1.0).abs() < 0.2, "{filled} vs {rest}");
    }
}
