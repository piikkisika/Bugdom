# Phase 1 milestone: model and skeleton viewer

Compare the port's viewer with the original game's model debug screen. Note
anything that differs, with the skeleton or model file, the animation or
object number, and what you see.

## Running both

| | Original (official release from github.com/jorio/Bugdom/releases) | Port |
|---|---|---|
| Start | Hold **F1** while the game starts | `cargo run -p bugdom --bin viewer` |
| Skeletons ↔ model files | Space | Space |
| Next / previous file | Tab / Shift+Tab | Tab / Shift+Tab |
| Next / previous animation or object | Enter / Shift+Enter | Enter / Shift+Enter |
| Camera | Arrow keys | Arrow keys, mouse wheel to zoom |

The port lists skeletons alphabetically and shows each file's name; the
original goes in its own order, so match them up by name.

## Expected differences (not bugs)

- **Lighting and background.** The port uses Bevy's standard material and a
  plain light; the original's lighting, fog and level colours come in Phase 2.
- **Camera framing.** The port frames each subject automatically.
- **Normals on a few creatures.** The original sometimes lights a vertex with
  the wrong bone's normal; the port does not.

## 1. Skeleton shape and texture

For each, compare proportions, colours, where the texture sits, and whether
any part is missing, extra, detached or stretched.

- [ ] Ant
- [ ] Buddy (the companion ladybug)
- [ ] DoodleBug (the player): its model file and rig differ in scale; the
      port draws the rig, as the original does, so its size should match
- [ ] DragonFly
- [ ] LadyBug
- [ ] QueenBee
- [ ] Spider
- [ ] RootSwing (the only skeleton whose texture repeats rather than clamps)

## 2. Animation timing

Time these against the original with a stopwatch, or count loops over ten
seconds. The port's label shows the tick (30 ticks per second); "stopped"
appears when an animation ends.

- [ ] **Ant #1 Standing**: one loop takes 1 second (30 ticks).
- [ ] **Ant #3 ThrowSpear**: plays once and stops at the rest pose; the label
      shows flag 0 set at tick 12 (the spear release).
- [ ] **Ant #5 FallOnButt**: plays forward to tick 13, then rocks back and
      forth between ticks 7 and 13 (marker and zig-zag).
- [ ] **Ant #7 Die**: plays through once, then loops ticks 11–61.
- [ ] **Spider #3 Walk**: loops ticks 6–41 after the first pass.
- [ ] **DoodleBug #5 Kick**: plays once (the kick sound is not wired up yet).
- [ ] **Bat #2 DiveBomb**: zig-zags back to the start.
- [ ] Easing: any animation where a limb visibly speeds up or slows down into
      a pose should look the same in both.

## 3. Model file objects

- [ ] **Lawn_Models1**: step through all 12 objects. Textures, UVs and
      vertex colours match.
- [ ] **MainMenu**: the signs' cut-out edges (alpha testing) look the same.
- [ ] **BeeHive_Models objects 20–23** (tubes, two meshes each): both parts
      present and textured.
- [ ] **Global_Models1** and **Global_Models2**: step through all objects.
- [ ] Any object with see-through parts (glass, water, wings) blends the same.

## 4. Anything else

Note crashes, objects that fail to load, or anything else that looks off.
