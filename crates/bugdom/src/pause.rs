//! Pausing a level.
//!
//! Port of the pause check in `PlayArea` (original/src/System/Main.c) and of
//! `DoPaused` (original/src/Screens/MiscScreens.c). Pausing stops virtual
//! time, so nothing in `FixedUpdate` runs and nothing timed by `Time`
//! advances, while the debug fly camera, which runs on real time, can still
//! frame a screenshot (as the original's `CheckPauseCameraKeys` allows). The
//! pause menu (resume, end the game, quit) arrives with the menus in
//! Phase 4; until then the pause key resumes.

use bevy::input::InputSystems;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use crate::input::{GameInputSystems, InputEnabled, InputSampler, grab_mouse, release_mouse};
use crate::state::{AppState, GameplayState};

/// Pauses and resumes (`kKey_Pause`, and `kKey_UI_Cancel` in the menu).
const PAUSE_KEY: KeyCode = KeyCode::Escape;
const PAUSE_BUTTON: GamepadButton = GamepadButton::Start;

pub struct PausePlugin;

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            toggle_pause
                .after(InputSystems)
                .before(GameInputSystems)
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(OnEnter(GameplayState::Paused), (stop_time, release_mouse))
        .add_systems(OnExit(GameplayState::Paused), start_time);
    }
}

/// Pauses on the pause key, and resumes on it again.
///
/// Resuming takes the mouse back and resets the input, as `PlayArea` does
/// after `DoPaused` returns, so the key that resumed doesn't also act in
/// the game.
fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    state: Res<State<GameplayState>>,
    enabled: Res<InputEnabled>,
    mut next: ResMut<NextState<GameplayState>>,
    mut sampler: ResMut<InputSampler>,
    mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let pressed =
        keys.just_pressed(PAUSE_KEY) || gamepads.iter().any(|pad| pad.just_pressed(PAUSE_BUTTON));
    if !pressed {
        return;
    }
    match state.get() {
        GameplayState::Playing => next.set(GameplayState::Paused),
        GameplayState::Paused => {
            next.set(GameplayState::Playing);
            sampler.reset();
            // The debug fly camera keeps the cursor to itself.
            if **enabled {
                for mut cursor in &mut cursors {
                    grab_mouse(&mut cursor);
                }
            }
        }
    }
}

fn stop_time(mut time: ResMut<Time<Virtual>>) {
    time.pause();
}

fn start_time(mut time: ResMut<Time<Virtual>>) {
    time.unpause();
}
