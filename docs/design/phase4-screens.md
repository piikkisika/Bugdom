# Phase 4 design: the screens and the game flow

Status: **draft, for review**. Items marked **[review]** change shared code
or need a decision.

Milestone: the game runs as the original does, from the logos through the
title screen and main menu, then each level (its intro, the level, the
bonus screen) to the win or lose screen and the high scores, with the pause
menu and the settings. After this, the deferred Phase 3 integration pass
plays the levels in that flow.

Work method: main session, one screen at a time, no subagents (PLAN.md).

## 1. The original's flow

`GameMain` (original/src/System/Main.c) runs the legal screen and the
Pangea logo once, then loops: `DoTitleScreen`, `DoMainMenu` (which also
leads to the settings, the about screens, the high scores and loading a
saved game), then `PlayGame`. `PlayGame` runs each level in turn:
`ShowLevelIntroScreen`, `InitArea`, `PlayArea`, `CleanupLevel`, then
`DoBonusScreen` (which offers to save), until the game is over; then
`DoWinScreen` or `DoLoseScreen`, and `ShowHighScoresScreen`.

Each screen is a blocking loop with its own 3D scene: it loads a model
file (`Title.3dmf`, `MainMenu.3dmf`, `LevelIntro.3dmf`, `BonusScreen.3dmf`,
`WinLose.3dmf`, `HighScores.3dmf`, `Pangea.3dmf`) and some skeletons, sets
up a camera and lights, moves its objects each frame and waits for a key,
a click or a timeout. Menu text is drawn with `TextMesh` (a bitmap font:
texture 3000 and an SFL glyph table).

## 2. States **[review]**

`AppState` grows from `Boot`, `Loading`, `InGame` to one state per screen,
each with `OnEnter` / `OnExit` (PLAN.md's object model mapping):

| State | Original |
|---|---|
| `Logos` | `DoLegalScreen`, `DoPangeaLogo` |
| `Title` | `DoTitleScreen` |
| `MainMenu` | `DoMainMenu` |
| `Settings`, `About`, `HighScores`, `FileSelect` | reached from the menu |
| `LevelIntro` | `ShowLevelIntroScreen` |
| `Loading`, `InGame` | as now (`InitArea`, `PlayArea`) |
| `Bonus` | `DoBonusScreen` |
| `Win`, `Lose` | `DoWinScreen`, `DoLoseScreen` |

`GameplayState::Paused` stays the pause menu's state (`DoPaused`).

What `PlayGame` keeps in globals becomes a `GameSession` resource: the
level (`gRealLevel`, today `CurrentLevel`), the score (`gScore`), and
whether the game is over or won. The players' inventory carrying over
between levels is the separate Phase 4 item and stays on the players.

The development shortcuts stay: `BUGDOM_LEVEL` (and the screenshot
variables) start straight in the level, skipping the screens, so captures
and the integration pass work as today.

## 3. Shared screen machinery

- **Screen assets.** A `ScreenAssets` load step per screen, like
  `LevelAssets`: its model file, skeletons and textures, loaded on
  `OnEnter` and waited for before the scene is built. The model and
  skeleton spawners take the screen's assets as well as a level's.
- **Screen scene.** Everything a screen spawns carries
  `DespawnOnExit(state)`; each screen has its camera, lights and fog,
  set as the original sets them.
- **Text.** A port of `TextMesh` (original/src/QD3D/TextMesh.c): parse the
  SFL glyph table, build a mesh per string with texture 3000. Used by the
  menus, the bonus screen, the high scores and the settings.
- **Picking and input.** The menus are clicked on 3D objects
  (`UpdateHoveredPickID`) or driven with the keys and pad. Clicking uses
  Bevy's own mesh picking (`bevy_picking`, part of Bevy: no new crate);
  keys and the pad go through `input.rs`'s actions.
- **Fades.** `GammaFadeOut`/`GammaFadeIn` become a full-screen fade
  overlay between states.

## 4. Order of work

Each step lands as its own commits with tests and a screenshot.

1. The state flow with the `GameSession`, each new screen a placeholder
   that moves on after a key or a moment. The game becomes playable end to
   end at once.
2. `TextMesh`, screen assets and the fade.
3. Title screen; main menu (with the spider that walks to the icons).
4. Level intro: the dropping level name first, then each level's intro
   scene (`DoLawn1Intro` … `DoAntHill1Intro`).
5. Bonus screen and the score: the tallies and the save prompt (saving
   itself comes with save games).
6. Win and lose screens; high scores with name entry.
7. Pause menu (completing `pause.rs`), settings, about and legal screens,
   the Pangea logo, and the level select cheat (F10).
8. Save games and file select, with the per-player inventory that lasts
   between levels.

Sound and music, input remapping and modding stay the later Phase 4
items.

## 5. Open questions **[review]**

1. **Where files go.** High scores, settings and saved games need a
   per-user directory. Bevy has no helper for it; the `dirs` crate is the
   usual choice (a new dependency), or a fixed path next to the game.
   This only matters from step 6.
2. **Screen resolution.** The original draws the screens at 640×480,
   stretched; Iliyan Jorio's port keeps their 4:3 framing on widescreen.
   Proposed: frame the 3D screens for 4:3 and show more to the sides, as
   the levels do (Hor+).
