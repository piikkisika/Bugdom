//! Game input: the original's key bindings, sampled every frame and handed
//! to the fixed timestep.
//!
//! Devices are read in `PreUpdate`. At the start of each fixed tick, every
//! player that this machine's devices control ([`LocalControls`]) gets a
//! [`ControlInput`] snapshot: the actions held at the last frame, every press
//! since the previous tick (so a press between ticks is not lost), and the
//! mouse motion summed since the previous tick. Each player carries its own
//! input and [`ControlSettings`], so more players can be added later.
//!
//! Port of original/src/System/Input.c.

use bevy::input::InputSystems;
use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::state::{AppState, GameplayState};

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputSampler>()
            .init_resource::<InputEnabled>()
            .add_systems(
                PreUpdate,
                (capture_mouse, sample_input)
                    .chain()
                    .in_set(GameInputSystems)
                    .after(InputSystems)
                    .run_if(in_state(GameplayState::Playing)),
            )
            .add_systems(OnExit(AppState::InGame), release_mouse)
            .add_systems(FixedPreUpdate, begin_tick);
    }
}

/// Reads the devices for the players each frame, in `PreUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GameInputSystems;

/// The game's actions (`kKey_*` in original/src/Headers/input.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Action {
    MorphPlayer,
    /// Sends the buddy bug at the nearest enemy.
    BuddyAttack,
    Jump,
    Kick,
    AutoWalk,
    Forward,
    Backward,
    Left,
    Right,
    ZoomIn,
    ZoomOut,
    SwivelCameraLeft,
    SwivelCameraRight,
}

/// The inputs bound to one action (`KeyBinding`).
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub action: Action,
    pub keys: &'static [KeyCode],
    pub mouse: Option<MouseButton>,
    pub gamepad: Option<GamepadButton>,
}

const fn bind(
    action: Action,
    keys: &'static [KeyCode],
    mouse: Option<MouseButton>,
    gamepad: Option<GamepadButton>,
) -> Binding {
    Binding {
        action,
        keys,
        mouse,
        gamepad,
    }
}

/// Jump and kick sit on the modifier keys, as in the original: Command and
/// Option on macOS, Alt and Control elsewhere.
#[cfg(target_os = "macos")]
const JUMP_KEYS: &[KeyCode] = &[KeyCode::SuperLeft, KeyCode::SuperRight];
#[cfg(not(target_os = "macos"))]
const JUMP_KEYS: &[KeyCode] = &[KeyCode::AltLeft, KeyCode::AltRight];
#[cfg(target_os = "macos")]
const KICK_KEYS: &[KeyCode] = &[KeyCode::AltLeft, KeyCode::AltRight];
#[cfg(not(target_os = "macos"))]
const KICK_KEYS: &[KeyCode] = &[KeyCode::ControlLeft, KeyCode::ControlRight];

/// The default bindings (`gKeyBindings`).
pub const DEFAULT_BINDINGS: [Binding; 13] = [
    bind(
        Action::MorphPlayer,
        &[KeyCode::Space],
        Some(MouseButton::Middle),
        Some(GamepadButton::East),
    ),
    bind(
        Action::BuddyAttack,
        &[KeyCode::Tab],
        None,
        Some(GamepadButton::North),
    ),
    bind(
        Action::Jump,
        JUMP_KEYS,
        Some(MouseButton::Right),
        Some(GamepadButton::South),
    ),
    bind(
        Action::Kick,
        KICK_KEYS,
        Some(MouseButton::Left),
        Some(GamepadButton::West),
    ),
    bind(
        Action::AutoWalk,
        &[KeyCode::ShiftLeft, KeyCode::ShiftRight],
        None,
        None,
    ),
    bind(
        Action::Forward,
        &[KeyCode::ArrowUp, KeyCode::KeyW],
        None,
        Some(GamepadButton::DPadUp),
    ),
    bind(
        Action::Backward,
        &[KeyCode::ArrowDown, KeyCode::KeyS],
        None,
        Some(GamepadButton::DPadDown),
    ),
    bind(
        Action::Left,
        &[KeyCode::ArrowLeft, KeyCode::KeyA],
        None,
        Some(GamepadButton::DPadLeft),
    ),
    bind(
        Action::Right,
        &[KeyCode::ArrowRight, KeyCode::KeyD],
        None,
        Some(GamepadButton::DPadRight),
    ),
    bind(
        Action::ZoomIn,
        &[KeyCode::Digit2],
        None,
        Some(GamepadButton::LeftTrigger),
    ),
    bind(
        Action::ZoomOut,
        &[KeyCode::Digit1],
        None,
        Some(GamepadButton::RightTrigger),
    ),
    bind(Action::SwivelCameraLeft, &[KeyCode::Comma], None, None),
    bind(Action::SwivelCameraRight, &[KeyCode::Period], None, None),
];

/// A set of actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionSet(u32);

impl ActionSet {
    pub fn contains(self, action: Action) -> bool {
        self.0 & (1 << action as u8) != 0
    }

    pub fn insert(&mut self, action: Action) {
        self.0 |= 1 << action as u8;
    }

    fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// A player's control preferences (part of the original's `PrefsType`).
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlSettings {
    /// The movement keys turn the bug and push it forward or back, rather
    /// than steering relative to the camera (`playerRelativeKeys`, off by
    /// default).
    pub player_relative_keys: bool,
}

/// A player controlled by this machine's keyboard, mouse and gamepad.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalControls;

/// Whether the devices are read for the players. The machine has one set
/// of devices, so this is not per player. The debug fly camera
/// turns this off while it uses the same keys.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Deref, DerefMut)]
pub struct InputEnabled(pub bool);

impl Default for InputEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// What this machine's devices said this frame, plus what has built up since
/// the last fixed tick.
#[derive(Resource, Debug, Default)]
pub struct InputSampler {
    held: ActionSet,
    /// Actions that were down when the input was reset, which don't count
    /// until they are released (`KEYSTATE_IGNOREHELD`).
    ignored: ActionSet,
    /// Presses since the last fixed tick (`KEYSTATE_PRESSED`, latched).
    pressed: ActionSet,
    /// Mouse motion since the last fixed tick, in pixels.
    mouse_motion: Vec2,
    left_stick: Vec2,
    right_stick: Vec2,
    /// Set when the mouse is captured, so that the click that captured it
    /// does not count as a press (`KEYSTATE_IGNOREHELD`).
    ignore_mouse_buttons: bool,
}

impl InputSampler {
    /// Forgets everything that has built up, and ignores whatever is down
    /// now until it is released, so that a key or button still held from a
    /// menu does not act in the game. Port of `ResetInputState`.
    pub fn reset(&mut self) {
        self.ignored = self.ignored.union(self.held);
        self.held = ActionSet::default();
        self.pressed = ActionSet::default();
        self.mouse_motion = Vec2::ZERO;
        self.left_stick = Vec2::ZERO;
        self.right_stick = Vec2::ZERO;
        self.ignore_mouse_buttons = true;
    }

    /// Takes in which actions the devices hold this frame: new ones count as
    /// presses, and ignored ones count again once released.
    fn update_held(&mut self, down: ActionSet) {
        self.ignored = self.ignored.intersection(down);
        let held = down.difference(self.ignored);
        self.pressed = self.pressed.union(held.difference(self.held));
        self.held = held;
    }
}

/// The input a player acts on during one fixed tick.
#[derive(Component, Debug, Default, Clone)]
pub struct ControlInput {
    held: ActionSet,
    pressed: ActionSet,
    /// Mouse motion since the previous tick, in pixels (down is positive).
    pub mouse_motion: Vec2,
    /// Gamepad thumbsticks after the dead zone, in `-1.0..=1.0`, with
    /// down positive like the mouse.
    pub left_stick: Vec2,
    pub right_stick: Vec2,
}

/// Width of the thumbsticks' dead zone (`kJoystickDeadZoneFrac`).
const STICK_DEAD_ZONE: f32 = 0.33;
/// Longest mouse movement one tick may use, in pixels (`MOUSE_DELTA_MAX`).
const MOUSE_DELTA_MAX: f32 = 250.0;
/// Steering rate of the movement keys and the left stick, in steering units
/// per second (the 1600 in `GetMouseDelta`).
const KEY_STEERING_RATE: f32 = 1600.0;
/// The mouse sensitivity setting's steering units per pixel: 1600 × the
/// default entry of `kMouseSensitivityTable`.
const MOUSE_STEERING_PER_PIXEL: f32 = 1600.0 * 0.05;
/// The original scales mouse steering by the frame time, which makes it
/// depend on the frame rate. We use its 60 fps value
/// (docs/design/phase2-engine-core.md §8).
const MOUSE_REFERENCE_FRAME_TIME: f32 = 1.0 / 60.0;
/// Camera swivel rate from the swivel keys and the right stick, in radians
/// per second (`gCameraControlDelta` in `UpdateInput`).
const SWIVEL_KEY_RATE: f32 = 2.0;
const SWIVEL_STICK_RATE: f32 = 3.0;

impl ControlInput {
    /// Whether the action is down (`GetKeyState`).
    pub fn held(&self, action: Action) -> bool {
        self.held.contains(action)
    }

    /// Whether the action was pressed since the previous tick
    /// (`GetNewKeyState`).
    pub fn just_pressed(&self, action: Action) -> bool {
        self.pressed.contains(action)
    }

    /// Whether the player is steering with the movement keys rather than the
    /// mouse (`gPlayerUsingKeyControl`).
    pub fn using_key_control(&self) -> bool {
        [
            Action::Forward,
            Action::Backward,
            Action::Left,
            Action::Right,
        ]
        .into_iter()
        .any(|action| self.held(action))
    }

    /// How hard the player steers this tick, in the original's units: x is
    /// right, y is backward. `dt` is the tick length in seconds.
    ///
    /// Port of `GetMouseDelta` (original/src/System/Input.c), including its
    /// quirk of capping the mouse movement only when it moves on both axes.
    pub fn steering(&self, dt: f32) -> Vec2 {
        if self.using_key_control() {
            let axis = |negative, positive| {
                if self.held(negative) {
                    -1.0
                } else if self.held(positive) {
                    1.0
                } else {
                    0.0
                }
            };
            return Vec2::new(
                axis(Action::Left, Action::Right),
                axis(Action::Forward, Action::Backward),
            ) * (KEY_STEERING_RATE * dt);
        }
        if self.left_stick != Vec2::ZERO {
            return self.left_stick * (KEY_STEERING_RATE * dt);
        }
        let mut motion = self.mouse_motion;
        if motion.x != 0.0 && motion.y != 0.0 && motion.length() > MOUSE_DELTA_MAX {
            motion = motion.normalize() * MOUSE_DELTA_MAX;
        }
        motion * (MOUSE_REFERENCE_FRAME_TIME * MOUSE_STEERING_PER_PIXEL)
    }

    /// How fast the player swivels the camera, in radians per second;
    /// positive turns it counter-clockwise seen from above
    /// (`gCameraControlDelta.x`).
    pub fn camera_swivel(&self) -> f32 {
        let mut swivel = -self.right_stick.x * SWIVEL_STICK_RATE;
        if self.held(Action::SwivelCameraLeft) {
            swivel -= SWIVEL_KEY_RATE;
        }
        if self.held(Action::SwivelCameraRight) {
            swivel += SWIVEL_KEY_RATE;
        }
        swivel
    }
}

#[cfg(test)]
impl ControlInput {
    /// Input with these actions held and pressed, for tests.
    pub fn for_tests(held: &[Action], pressed: &[Action]) -> Self {
        let mut input = Self::default();
        for &action in held {
            input.held.insert(action);
        }
        for &action in pressed {
            input.pressed.insert(action);
        }
        input
    }
}

/// Applies the dead zone to a thumbstick and rescales what is left to
/// `0.0..=1.0`. Port of `GetThumbStickVector`.
fn thumbstick(raw: Vec2) -> Vec2 {
    let magnitude = raw.length();
    if magnitude < STICK_DEAD_ZONE {
        return Vec2::ZERO;
    }
    let scaled = if magnitude > 1.0 {
        1.0
    } else {
        (magnitude - STICK_DEAD_ZONE) / (1.0 - STICK_DEAD_ZONE)
    };
    raw / magnitude * scaled
}

fn mouse_captured(cursor: &CursorOptions) -> bool {
    cursor.grab_mode != CursorGrabMode::None
}

/// Captures the mouse again on a click in the window, after it was
/// released by pausing or by leaving the window (`CaptureMouse`).
fn capture_mouse(
    enabled: Res<InputEnabled>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut sampler: ResMut<InputSampler>,
    mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if !**enabled {
        return;
    }
    let Ok(mut cursor) = cursors.single_mut() else {
        return;
    };
    if !mouse_captured(&cursor) && buttons.get_just_pressed().next().is_some() {
        grab_mouse(&mut cursor);
        sampler.ignore_mouse_buttons = true;
    }
}

/// Hides and locks the cursor, so that mouse motion steers the player
/// (`CaptureMouse(true)`).
pub fn grab_mouse(cursor: &mut CursorOptions) {
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}

/// Gives the cursor back (`CaptureMouse(false)`).
pub fn release_mouse(mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    for mut cursor in &mut cursors {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Reads the devices. Port of `UpdateInput` and `UpdateKeyMap`.
fn sample_input(
    enabled: Res<InputEnabled>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    gamepads: Query<&Gamepad>,
    windows: Query<(&Window, &CursorOptions), With<PrimaryWindow>>,
    mut sampler: ResMut<InputSampler>,
) {
    let (focused, captured) = windows.single().map_or((false, false), |(window, cursor)| {
        (window.focused, mouse_captured(cursor))
    });
    if !**enabled || !focused {
        // Keys held while the window was away must be released before they
        // count again.
        sampler.reset();
        return;
    }

    if sampler.ignore_mouse_buttons && buttons.get_pressed().next().is_none() {
        sampler.ignore_mouse_buttons = false;
    }
    let use_mouse = captured && !sampler.ignore_mouse_buttons;
    let gamepad = gamepads.iter().next();

    let mut held = ActionSet::default();
    for binding in &DEFAULT_BINDINGS {
        let down = binding.keys.iter().any(|&key| keys.pressed(key))
            || (use_mouse && binding.mouse.is_some_and(|b| buttons.pressed(b)))
            || gamepad.is_some_and(|pad| binding.gamepad.is_some_and(|b| pad.pressed(b)));
        if down {
            held.insert(binding.action);
        }
    }
    sampler.update_held(held);

    if captured {
        sampler.mouse_motion += motion.delta;
    }
    let stick = |x, y| {
        gamepad.map_or(Vec2::ZERO, |pad| {
            // Bevy's stick y points up; the original's (SDL's) points down.
            let raw = Vec2::new(pad.get(x).unwrap_or(0.0), -pad.get(y).unwrap_or(0.0));
            thumbstick(raw)
        })
    };
    sampler.left_stick = stick(GamepadAxis::LeftStickX, GamepadAxis::LeftStickY);
    sampler.right_stick = stick(GamepadAxis::RightStickX, GamepadAxis::RightStickY);
}

/// Hands what has built up since the last tick to the locally controlled
/// players for this tick.
fn begin_tick(
    mut sampler: ResMut<InputSampler>,
    mut players: Query<&mut ControlInput, With<LocalControls>>,
) {
    let input = ControlInput {
        held: sampler.held,
        pressed: std::mem::take(&mut sampler.pressed),
        mouse_motion: std::mem::take(&mut sampler.mouse_motion),
        left_stick: sampler.left_stick,
        right_stick: sampler.right_stick,
    };
    for mut player_input in &mut players {
        *player_input = input.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_holding(actions: &[Action]) -> ControlInput {
        ControlInput::for_tests(actions, &[])
    }

    #[test]
    fn keys_steer_at_a_fixed_rate() {
        let input = input_holding(&[Action::Forward, Action::Right]);
        assert!(input.using_key_control());
        let steering = input.steering(0.5);
        assert_eq!(steering, Vec2::new(800.0, -800.0));
    }

    #[test]
    fn mouse_motion_is_capped_only_when_diagonal() {
        let mut input = ControlInput {
            mouse_motion: Vec2::new(1000.0, 0.0),
            ..default()
        };
        let scale = MOUSE_REFERENCE_FRAME_TIME * MOUSE_STEERING_PER_PIXEL;
        assert_eq!(input.steering(1.0), Vec2::new(1000.0 * scale, 0.0));
        input.mouse_motion = Vec2::new(300.0, 400.0);
        let capped = input.steering(1.0) / scale;
        assert!(capped.abs_diff_eq(Vec2::new(150.0, 200.0), 1e-3));
    }

    #[test]
    fn keys_held_through_a_reset_count_once_released() {
        let mut sampler = InputSampler::default();
        let mut jump = ActionSet::default();
        jump.insert(Action::Jump);
        sampler.update_held(jump);
        sampler.reset();
        sampler.update_held(jump);
        assert!(!sampler.held.contains(Action::Jump));
        assert!(!sampler.pressed.contains(Action::Jump));
        sampler.update_held(ActionSet::default());
        sampler.update_held(jump);
        assert!(sampler.held.contains(Action::Jump));
        assert!(sampler.pressed.contains(Action::Jump));
    }

    #[test]
    fn thumbstick_dead_zone_rescales() {
        assert_eq!(thumbstick(Vec2::new(0.3, 0.0)), Vec2::ZERO);
        assert_eq!(thumbstick(Vec2::new(1.0, 0.0)), Vec2::new(1.0, 0.0));
        let half = thumbstick(Vec2::new(0.0, 0.665));
        assert!((half.y - 0.5).abs() < 1e-6);
    }
}
