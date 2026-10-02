# Phase 4 design: the in-game HUD (infobar)

Status: **approved** (2026-10-02), as drafted, with these decisions:

- The boss bar follows a shared `BossHealthBar { full }` component (§8).
- **Responsive positioning where possible, keeping the original layout.**
  The bars, the band between them and the elements are laid out with
  nested flex nodes (§4.1), rather than every element at absolute
  640×480 coordinates.
- **Idiomatic Bevy UI over the C mechanics.** The original's compositing
  texture, `DrawSprite`/`EraseSprite` and `gInfobarUpdateBits` are not
  replicated; what they draw is. Option B (§4) is dropped.

Milestone: every level shows the original's infobar. It updates during
play and scales to any window.

## 1. Goal

This phase ports the drawing half of original/src/Screens/Infobar.c. The
inventory half (`GetKey`, `LoseHealth`, …) is already in
`player/inventory.rs`, `player/health.rs` and `player/ball.rs`. The HUD
should look and behave like the original at 4:3, and scale to widescreen as
Iliyan Jorio's port does. It reads one player's components, so that each of
several players can later have a HUD of its own.

## 2. How the original draws it

- **Layout.** Everything is laid out on a 640×480 screen:
  - the top bar, 640×62 at y = 0;
  - the bottom bar, 640×60 at y = 420;
  - the 3D view, clipped to the band between them (`paneClip` in
    `InitArea`, Main.c). Its aspect ratio comes from that band, so the
    vertical field of view of 1.1 rad spans only 358 of the 480 rows.
- **Compositing.** `InitInfobar` draws both background sprites into one
  1024×128 texture. `DrawSprite` draws each element into it on the CPU,
  treating pure black as transparent. `EraseSprite` copies the background
  back. `SubmitInfobarOverlay` uploads the parts that changed, then draws
  two quads through `glOrtho(0, 640, 480, 0)`.
- **Updates.** Gameplay code sets bits in `gInfobarUpdateBits`.
  `UpdateInfobar` runs once per frame, after the objects and the camera
  have moved, and redraws only the elements whose bits are set.
  `InitInfobar` sets every bit at the start of each area.
- **No animation.** Nothing in the infobar blinks, flashes or tweens. It
  only swaps sprites or redraws a value. The shield, invincibility and the
  torch are not shown in it.
- **Bottom bar setting.** `gGamePrefs.showBottomBar` is on by default. With
  it off, the 3D view reaches the bottom of the screen and only the 200×20
  boss strip is drawn, and only on the three boss levels
  (`gBossHealthWasUpdated`).
- **Widescreen** (Renderer.c: `Render_Enter2D_Full640x480`,
  `Render_GetAdjustedViewportRect`).
  - By default the 640×480 overlay is **stretched** to fill the window. On
    16:9 the bars are a third wider and keep 62/480 and 60/480 of the
    height.
  - The 3D band uses the full width, so widescreen shows more to the sides
    (Hor+).
  - `force4x3AspectRatio` fits both the overlay and the 3D view to 4:3 and
    pillarboxes them.
  - The band's top edge is rounded down and its bottom edge up, so that no
    seam shows between the bars and the 3D view.

## 3. Elements

Positions are (x, y, w, h) in 640×480 units. Sprite *n* of the `SPRITE_*`
enum is `Data/Images/Infobar/{128+n}.tga`. These files are 16-bit, opaque
(descriptor `0x20`), with black used as transparent.

| Element | Original | Where | Sprites | Data in the port | Redrawn on |
|---|---|---|---|---|---|
| Top bar | `InitInfobar` | (0, 0, 640, 62) | 157 | none | area start |
| Bottom bar | `InitInfobar` | (0, 420, 640, 60) | 158 | none | area start |
| Spare lives | `ShowLives` | icon *i* at (11 + 42*i*, 0) | 128–130 (three different pictures) | `Inventory::lives`: icon *i* shows if *i* < lives − 1 | death reset, free-life nut |
| Gold clover | `ShowGoldClover` | (141, 0, 55, 59) | 131–134 for 1, 2, 3 and 4 or more; hidden at 0 | `Inventory::gold_clovers` | `GetGoldClover` |
| Blue clover | `ShowBlueClover` | (196, 0, 54, 59) | 135–138, the same way | `Inventory::blue_clovers` | `GetBlueClover` |
| Ball-time gauge | `DrawNitroGauge` | (275, 6, 90, 41) | `NitroGauge.tga` (8-bit template) | `BallTime` | `LoseBallTime`, ball-time nut |
| Hands (bug) | `ShowBallTimerAndInventory` | left at (320 − w, 1), right at (320, 1) | empty 142/143, money 144/145, keys green to purple 146–155 (L, R) | `Inventory::hands`, `PlayerForm` | `GetKey`, `UseKey`, `GetMoney`, `UseMoney`, form change |
| Ball icon | `ShowBallTimerAndInventory` | (307, 17, 27, 26) | 156 | `PlayerForm::Ball` | form change |
| Health | `ShowHealth` | (274, 50, 92, 7) | filled rectangles | `Health` | `LoseHealth`, `GetHealth`, `ResetPlayer` |
| Ladybug icon | `ShowLadyBugs` | (396, 0, 74, 62) | 139 if some are left, 140 if all are freed | `Inventory::has_all_ladybugs()` | `GetLadyBug` |
| Freed ladybugs | `ShowLadyBugs` | ladybug *i* at x = 476 + 22⌊*i*/2⌋, y = 7 (even *i*) or 30 (odd *i*) | 141 | `Inventory::ladybugs` | `GetLadyBug` |
| Boss health | `ShowBossHealth` | frame (220, 440, 200, 20) | filled rectangles | the boss's `Health` (§8) | hive hit, queen or king hurt |

Details an exact port needs:

- **Gauge.** A template pixel is either 0, meaning outside the gauge, or
  1 + an angle from 0 to 180°. Let `n = ⌊180 × ball time⌋`. A pixel at
  angle *t* is drawn:
  - green `#00BD29` if *t* ≤ *n*;
  - otherwise yellow `#FFF700` if *t* ≤ *n* + 3 and 2 < *n* < 178, which
    makes the margin line;
  - otherwise black.

  The gauge is redrawn for a timer change only once *n* has moved by 2 or
  more (`gOldTimerN`). The hands are drawn over the gauge. In ball form,
  the ball icon covers the bug's head.
- **Health.** Let `n = ⌊92 × health⌋`. The bar is red `#F70018` for *n* px
  from x = 274, then black. A yellow line 2 px wide marks the boundary,
  unless it would fall within 2 px of the right end.
- **Boss bar.** The frame is black. Let `w = ⌊200 × health⌋`. The fill is
  `#DD0806`, starting at (222, 442), `w − 4` wide and 16 high, with black
  after it. The health is clamped at 0. It is:
  - `hive.Health` on Dragonfly Attack (1.0, minus 0.02 per hit);
  - `queen.Health / 7` on Queen Bee;
  - `king.Health / 5` on Ant King.

  Before the boss exists, the bar is full. The killing blow updates it,
  because `KillQueenBee` and `KillKingAnt` return false, so the bar ends
  empty.
- **Hands in ball form.** `GetKey` and `GetMoney` skip the redraw in ball
  form, but morphing back redraws. So drawing the hands from `Inventory`
  whenever the player is in `Bug` form is exact.
- **Ladybug overflow.** Only 14 small ladybugs fit before x = 640. The
  rest fall into the hidden part of the texture.

## 4. Rendering approach **[review]**

- **A. Bevy UI, one node per element (recommended).**
  - Sprites are `ImageNode`s, and the health and boss rectangles are
    `BackgroundColor`s.
  - A helper turns 640×480 coordinates into `Val::Percent` of the HUD
    root.
  - "Erase" becomes `Visibility::Hidden`, and a sprite swap sets
    `ImageNode::image`.
  - Plain handles are enough: a `TextureAtlas` would only add a packing
    step for 31 small images.
  - The gauge is an `ImageNode` whose own 90×41 image is rewritten on the
    CPU. A `UiMaterial` shader could do the same, but would add WGSL for
    no gain.
- **B. A CPU canvas, as the original does.** `DrawSprite` and `EraseSprite`
  are ported onto a 640×122 image for each HUD. This is pixel-exact, but it
  is a framebuffer in disguise, hard to mod, and it re-uploads the whole
  bar on every change.
- **C. A 2D camera with `Sprite`s**, using a projection fixed at 640×480
  (`ScalingMode::Fixed`). The stretch comes out exact, but each player's
  HUD would need its own `RenderLayers`, and the Phase 4 menus will use
  Bevy UI anyway.

A is the idiomatic choice. It gives one entity per element with change
detection on each, its layout could later move into data for mods,
`UiTargetCamera` places it in a view, and the menus will share the same
toolkit. The workspace's `"ui"` feature already enables `bevy_ui`,
`bevy_ui_render` and `bevy_sprite_render` (checked in bevy 0.19.1's
Cargo.toml), so no feature changes are needed. The step 1 screenshots
need to check two risks:

- **Dark fringes.** With bilinear filtering, a scaled-up sprite blends its
  transparent black texels into its edges. The loader prevents this by
  copying neighbouring colours into those texels, with their alpha left
  at 0 (§8).
- **Pixel snapping.** At scales that are not whole numbers, each node
  rounds to physical pixels on its own. If a sprite visibly sits a pixel
  off its background, the fallback is B for the top bar.

**Cameras.** Bevy UI lays a root out inside its camera's viewport, so the
HUD can't target the 3D camera once that camera is clipped to the band.

- `GameCamera` (order 0) gets a `Camera::viewport` for the band between
  the bars. This is the port of `paneClip` and
  `Render_GetAdjustedViewportRect`, including the rounding, and it is
  recomputed when the window or the settings change. The view is then
  framed as the original frames it. Today the port draws 3D over the whole
  window, so the Phase 2 camera checks should be repeated.
- A `HudCamera` (`Camera2d`, order 1, `ClearColorConfig::None`) covers the
  whole view. Each HUD root carries `UiTargetCamera`. With split screen,
  each view gets its own `HudCamera`, and because the HUD's positions are
  percentages it fits any region unchanged.
- The fade overlay that comes later must sit above the HUD
  (`GlobalZIndex`), because the original draws `Render_DrawFadeOverlay`
  after `SubmitInfobarOverlay`.

### 4.1 Responsive layout (owner's decision)

The original's look is kept: the same art, the same places on it, the
same proportions at 4:3. How the nodes get there is Bevy's:

- **Screen.** A root node fills the HUD camera's view as a column: the top
  bar, the game band (`flex_grow: 1`) and the bottom bar. The bars' heights
  are their share of the window height (62/480 and 60/480), and they span
  the full width, which gives the source port's widescreen stretch.
- **The 3D view follows the layout.** `fit_game_view` sets the game
  camera's viewport from the band node's computed size and position
  (`ComputedNode`, `UiGlobalTransform`), so the band is defined in one
  place. It keeps the original's rounding, which avoids a seam.
- **Elements live in their bar.** Each element is a child of its bar,
  positioned in percentages of that bar's size, so it stays on its spot
  of the background art at any window size. Positions come from the
  original's coordinates, converted once by a helper.
- **Flex where the art allows.** Groups that are only a row of repeated
  sprites, such as the ladybugs and the lives, are flex rows with a gap,
  not one computed position per sprite. Text-like numbers are a row of
  digit images.
- **No dirty-rectangle logic.** Each element is its own node. A value
  change sets an image or a size, and Bevy redraws. Hiding is
  `Visibility::Hidden` or `Display::None`.

## 5. Scaling and aspect ratio **[review]**

- **Default: stretch, as the source port does.** Every element is a
  percentage of 640×480, drawn with `NodeImageMode::Stretch`, and the 3D
  band is Hor+. This matches the release the owner compares against.
- **Option: 4:3 pillarbox.** The root gets `aspect_ratio: Some(4.0 / 3.0)`
  and is centred, and the 3D viewport is fitted the same way. This arrives
  with the settings screen.
- **Not proposed: keeping the bars' proportions on widescreen.** The art
  is 640 px wide, so the sides would need new art.
- **Where the settings live.** `show_bottom_bar` and `force_4x3` go in a
  `DisplaySettings` resource. They are machine-wide, like `gGamePrefs`,
  rather than player preferences, so they are not components.

## 6. Which player a HUD shows

Like the camera's `CameraTarget(Entity)`, a `HudTarget(Entity)` names the
player.

- It goes on the HUD root and is copied onto each element at spawn. Each
  element system is then one query plus `players.get(target.0)`, with no
  `single()`.
- `spawn_hud` runs `OnEnter(AppState::InGame)`, after
  `PlayerSystems::Spawn`. It spawns one HUD for each `Player` with
  `LocalControls`.
- Every HUD entity carries `DespawnOnExit(AppState::InGame)`, so the HUD is
  rebuilt for each area, as `InitInfobar` does.
- The boss bar shows a level-wide value, and every HUD shows it.

## 7. Plugin and systems

There is one `HudPlugin`, in a new `hud/` module:

| File | Contents |
|---|---|
| `mod.rs` | plugin, `HudTarget`, `HudSystems`, `spawn_hud`, the 640×480 helper |
| `art.rs` | `InfobarArt`: the sprite handles and the gauge template |
| `view.rs` | `HudCamera`, `DisplaySettings`, `fit_game_view` |
| `status.rs` | health and lives |
| `gauge.rs` | the ball-time gauge, the hands and the ball icon |
| `pickups.rs` | the ladybugs and the clovers |
| `boss.rs` | the boss bar |

The systems run in `Update`, in `HudSystems::Refresh`, while in
`AppState::InGame`. Each has one job and cites its C original:

| System | Port of | Works when |
|---|---|---|
| `fit_game_view` | `paneClip`, `Render_GetAdjustedViewportRect` | the window is resized, or `DisplaySettings` changes |
| `show_health` | `ShowHealth` | `Health` changed |
| `show_lives` | `ShowLives` | `Inventory` changed |
| `show_clovers` | `ShowGoldClover`, `ShowBlueClover` | `Inventory` changed |
| `show_ladybugs` | `ShowLadyBugs` | `Inventory` changed |
| `show_hands` | the hands in `ShowBallTimerAndInventory` | `Inventory` or `PlayerForm` changed |
| `draw_ball_gauge` | `DrawNitroGauge` | `BallTime` moved 2° or more, or `PlayerForm` changed |
| `show_boss_health` | `ShowBossHealth` | the boss's `Health` changed |
| `show_bottom_bar` | the bottom-bar choice in `SubmitInfobarOverlay` | `DisplaySettings` changed |

- **Change detection.** `Update` sees the changes that `FixedUpdate` made
  through the usual change ticks, however many fixed steps ran. An
  element also draws when it has just been added (`Ref::is_added`), which
  replaces `InitInfobar` setting every bit. Writes use `set_if_neq`, so an
  unchanged value does not trigger a UI layout pass.
- **Boss bar.**
  - It starts full on the levels with `LevelDef::is_boss_level`, which
    are exactly the three in `ShowBossHealth`.
  - It follows the boss's health only when that changes on a boss that
    was already there (`is_changed() && !is_added()`). So a respawned boss
    doesn't refill it before its first hit, and a despawned one leaves it
    as it was, both as in the original.
- **Tests.** The numbers go in pure helpers, tested without rendering:
  `health_bar`, `boss_fill_width`, `gauge_color`, `ladybug_position`,
  `lives_shown`, `clover_sprite` and `game_band`. One more test draws the
  gauge from the real `NitroGauge.tga`. Constants keep the original's names
  (`HEALTH_X`, `BOSS_WIDTH`, …), in 640×480 units.

## 8. Assets and gaps

- **Parsers: no gap.** `bugdom_formats::tga` already decodes the 16-bit
  sprites and the 8-bit greyscale gauge; its tests open `NitroGauge.tga`.
  `TgaLoader` copies a grey value into R, G and B, so the template value
  survives exactly. Images keep their CPU-side pixels, because
  `RenderAssetUsages::default()` includes `MAIN_WORLD`.
- **Black as transparent.** `TgaLoader` takes no settings. The fences call
  `black_to_transparent` after loading. Proposed: a
  `TgaSettings { black_is_transparent, bleed_edges }` loader setting,
  loaded with `load_with_settings`, which edits the shared
  `assets/image.rs`. **[review]**
  - The two backgrounds are loaded without it, because the original's
    texture is opaque.
  - The HUD sprites use `ClampToEdge`, as `kRendererTextureFlags_ClampBoth`
    does.
- **Loading.** `InfobarArt` loads at startup, as `LoadInfobarArt` loads at
  boot. `finish_loading` should also wait for it: one line in `state.rs`.
- **Boss reference (decided).** A generic `BossHealthBar { full: f32 }`
  component goes on whatever the bar follows. The queen bee and king ant
  plugins and the hive item each add it, and the HUD shows
  `Health / full`. It is a new shared component, defined in `combat.rs`
  next to `Health`, so Phase 3 packages can add it before the HUD exists.
- **Cheats.** Of `CheckForCheats`, only F1 is ported. F3 (health), F4 (ball
  time) and F5 (keys and money) would let the screenshot checks exercise
  the HUD before the Phase 3 pickups exist.

## 9. Intentional differences **[review]**

1. **Sprite edges are filtered one sprite at a time** (option A), where
   the original filters the composited bar as a whole. Edges come out
   slightly softer at large scales.
2. **Not a difference, but a visible change from the port today:** the 3D
   view shrinks to the band between the bars, as in the original.

Everything else is kept: the colours, the positions, the 2° gauge
threshold and the boss bar's behaviour.

## 10. Order of work

1. **Foundation (main session).**
   - `HudPlugin`, `HudTarget` and the 640×480 helper.
   - `InfobarArt` and the loader setting.
   - `HudCamera` and `fit_game_view`.
   - `spawn_hud` with the two backgrounds.
   - The F3 to F5 cheats.

   Check with screenshots at 640×480, 1280×720 and 1920×1080.
2. **Fan-out**, in parallel worktrees, one module each, following the
   rules in Phase 3 §3:
   - a. `status.rs`
   - b. `gauge.rs`
   - c. `pickups.rs`
   - d. `boss.rs`: needs the boss reference decision; it can be tested
     with a stand-in entity that has `Health`.
   - e. `DisplaySettings`, the bottom-bar toggle and 4:3: can wait for the
     settings screen.

   Each package adds one registration line, its tests and a screenshot.
3. **Integration.** Screenshots in bug and ball form and on the three boss
   levels. A short checklist for the owner covers the bar positions, the
   gauge sweep, the health line and the boss bar going from full to empty.

## 11. Dependencies

- **From Phase 3:**
  - **Boss references (§8).** The queen bee and king ant plugins, the
    hive item (package E) and the dragonfly's hits on the hive
    (package F).
  - **Inventory changes.** The nut's contents (package B) give keys,
    money, clovers and lives. The ladybug cage (package B) frees ladybugs.
    The doors and the water bug taxi (package G) use up keys and money.

  Until those exist, the HUD's values simply never change, and the cheats
  cover the tests.
- **From Phase 4:**
  - **Inventory kept between levels.** Lives, gold clovers and ball time
    carry over in the original. Until this item lands, they reset at
    every level.
  - **The settings screen** (bottom bar, 4:3).
  - **The fades**, which must draw above the HUD.
