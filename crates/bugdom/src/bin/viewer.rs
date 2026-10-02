//! Model and skeleton viewer, the equivalent of the original's model debug
//! screen (original/src/Screens/ModelDebug.c). Used to check the parsers and
//! loaders against the original game.
//!
//! Controls (the same keys as the original's screen where it has them):
//! - Space: switch between skeletons and model files
//! - Tab / Shift+Tab: next / previous skeleton or model file
//! - Enter / Shift+Enter: next / previous animation or object
//! - Left/right arrows: orbit; up/down arrows: tilt; mouse wheel: zoom
//! - P: pause
//!
//! For automated checks, `BUGDOM_VIEWER_CAPTURE=<kind>:<name>:<item>:<png>`
//! (kind is `skeleton` or `model`) shows that selection, saves a screenshot
//! after a moment and exits. With `BUGDOM_VIEWER_CAPTURE_TICK=<tick>`, a
//! skeleton's animation is frozen at that tick for the screenshot.

use std::path::Path;

use bevy::camera::primitives::MeshAabb;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bugdom::assets::model::Model;
use bugdom::assets::skeleton::SkeletonAsset;
use bugdom::assets::{ORIGINAL_SOURCE, OriginalAssetsPlugin, OriginalDataSourcePlugin};
use bugdom::skeleton::{AnimationFlags, Skeleton, SkeletonAnimator, SkeletonPlugin, SkeletonRig};

fn main() -> AppExit {
    App::new()
        .add_plugins(OriginalDataSourcePlugin)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bugdom model viewer".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((OriginalAssetsPlugin, SkeletonPlugin))
        .insert_resource(GlobalAmbientLight {
            brightness: 400.0,
            ..default()
        })
        .add_systems(Startup, (setup, capture::setup).chain())
        .add_systems(Update, capture::run)
        .add_systems(
            Update,
            (
                handle_keys,
                respawn_subject,
                frame_subject,
                orbit_camera,
                update_label,
            )
                .chain(),
        )
        .run()
}

/// What the viewer can show, discovered from the data directory.
#[derive(Resource)]
struct Catalogue {
    skeletons: Vec<String>,
    models: Vec<String>,
}

#[derive(Resource, Default, PartialEq, Clone, Copy, Debug)]
enum Mode {
    #[default]
    Skeletons,
    Models,
}

/// The current selection. Changing it respawns the subject.
#[derive(Resource, Default)]
struct Selection {
    mode: Mode,
    file: usize,
    /// Animation number (skeletons) or object group (models).
    item: usize,
    paused: bool,
}

/// The skeleton or model being shown.
#[derive(Component)]
struct Subject;

#[derive(Component)]
struct Label;

/// Orbit camera state.
#[derive(Component)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
    framed: bool,
}

fn list_files(dir: &str, suffix: &str) -> Vec<String> {
    let path = bugdom_formats::original_data_dir().join(dir);
    let mut names: Vec<String> = std::fs::read_dir(&path)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter_map(|n| n.strip_suffix(suffix).map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|e| {
            error!("cannot list {}: {e}", Path::new(&path).display());
            Vec::new()
        });
    names.sort();
    names
}

fn setup(mut commands: Commands) {
    commands.insert_resource(Catalogue {
        skeletons: list_files("Skeletons", ".skeleton.rsrc"),
        models: list_files("Models", ".3dmf"),
    });
    commands.init_resource::<Selection>();
    commands.spawn((
        Camera3d::default(),
        Orbit {
            yaw: 0.0,
            pitch: -0.3,
            distance: 300.0,
            target: Vec3::ZERO,
            framed: false,
        },
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 8000.0,
            ..default()
        },
        Transform::from_xyz(1.0, 2.0, 1.5).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Label,
        Text::default(),
        Node {
            position_type: PositionType::Absolute,
            top: px(8),
            left: px(8),
            ..default()
        },
    ));
}

fn handle_keys(
    keys: Res<ButtonInput<KeyCode>>,
    catalogue: Res<Catalogue>,
    mut selection: ResMut<Selection>,
    mut time: ResMut<Time<Virtual>>,
) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let step = |n: usize, len: usize| {
        let len = len.max(1);
        if shift {
            (n + len - 1) % len
        } else {
            (n + 1) % len
        }
    };
    let file_count = match selection.mode {
        Mode::Skeletons => catalogue.skeletons.len(),
        Mode::Models => catalogue.models.len(),
    };

    if keys.just_pressed(KeyCode::Space) {
        selection.mode = match selection.mode {
            Mode::Skeletons => Mode::Models,
            Mode::Models => Mode::Skeletons,
        };
        selection.file = 0;
        selection.item = 0;
    }
    if keys.just_pressed(KeyCode::Tab) {
        selection.file = step(selection.file, file_count);
        selection.item = 0;
    }
    if keys.just_pressed(KeyCode::Enter) {
        // The item count is only known once loaded, so wrap in respawn_subject.
        selection.item = if shift {
            selection.item.wrapping_sub(1)
        } else {
            selection.item + 1
        };
    }
    if keys.just_pressed(KeyCode::KeyP) {
        selection.paused = !selection.paused;
        if selection.paused {
            time.pause();
        } else {
            time.unpause();
        }
    }
}

/// Respawns the subject when the selection changes, and applies the
/// animation or object choice once the asset is loaded.
fn respawn_subject(
    mut commands: Commands,
    selection: Res<Selection>,
    catalogue: Res<Catalogue>,
    assets: Res<AssetServer>,
    models: Res<Assets<Model>>,
    skeletons: Res<Assets<SkeletonAsset>>,
    subjects: Query<Entity, With<Subject>>,
    mut animators: Query<(&mut SkeletonAnimator, &Skeleton)>,
    mut orbits: Query<&mut Orbit>,
    mut shown: Local<Option<(Mode, usize, usize)>>,
    // Keeps the model file loaded: dropping the last strong handle while it
    // is loading would cancel the load.
    mut model_handle: Local<Option<Handle<Model>>>,
) {
    let current = (selection.mode, selection.file, selection.item);
    if *shown == Some(current) {
        return;
    }

    let same_file = shown.is_some_and(|(m, f, _)| (m, f) == (selection.mode, selection.file));
    if selection.mode == Mode::Skeletons && same_file {
        // Only the animation changed: keep the rig and restart the animation.
        for (mut animator, skeleton) in &mut animators {
            let Some(asset) = skeletons.get(&skeleton.0) else {
                return;
            };
            let count = asset.definition.animations.len().max(1);
            animator.set_anim(selection.item % count);
        }
        *shown = Some(current);
        return;
    }

    for entity in &subjects {
        commands.entity(entity).despawn();
    }
    for mut orbit in &mut orbits {
        orbit.framed = false;
    }

    match selection.mode {
        Mode::Skeletons => {
            let Some(name) = catalogue.skeletons.get(selection.file) else {
                return;
            };
            let handle: Handle<SkeletonAsset> = assets.load(format!(
                "{ORIGINAL_SOURCE}://Skeletons/{name}.skeleton.rsrc"
            ));
            let mut animator = SkeletonAnimator::default();
            animator.set_anim(selection.item);
            commands.spawn((Subject, Skeleton(handle), animator));
            *shown = Some(current);
        }
        Mode::Models => {
            let Some(name) = catalogue.models.get(selection.file) else {
                return;
            };
            let handle: Handle<Model> =
                assets.load(format!("{ORIGINAL_SOURCE}://Models/{name}.3dmf"));
            *model_handle = Some(handle.clone());
            // Wait for the model so the object number can wrap.
            let Some(model) = models.get(&handle) else {
                return;
            };
            let Some(group) = model.groups.get(selection.item % model.groups.len().max(1)) else {
                return;
            };
            commands
                .spawn((Subject, Transform::default(), Visibility::default()))
                .with_children(|parent| {
                    for &part in &group.parts {
                        let part = &model.parts[part];
                        parent.spawn((
                            Mesh3d(part.mesh.clone()),
                            MeshMaterial3d(part.material.clone()),
                        ));
                    }
                });
            *shown = Some(current);
        }
    }
}

/// Points the camera at the subject once its size is known.
fn frame_subject(
    selection: Res<Selection>,
    catalogue: Res<Catalogue>,
    assets: Res<AssetServer>,
    models: Res<Assets<Model>>,
    skeletons: Res<Assets<SkeletonAsset>>,
    meshes: Res<Assets<Mesh>>,
    subjects: Query<&Skeleton, (With<Subject>, With<SkeletonRig>)>,
    mut orbits: Query<&mut Orbit>,
) {
    let Ok(mut orbit) = orbits.single_mut() else {
        return;
    };
    if orbit.framed {
        return;
    }
    let (center, radius) = match selection.mode {
        Mode::Skeletons => {
            // Frame the bind-pose meshes; bone positions alone can be far
            // too tight (Buddy has three bones close together).
            let Ok(skeleton) = subjects.single() else {
                return;
            };
            let Some(asset) = skeletons.get(&skeleton.0) else {
                return;
            };
            let mut min = Vec3::splat(f32::MAX);
            let mut max = Vec3::splat(f32::MIN);
            for part in &asset.parts {
                let Some(aabb) = meshes.get(&part.mesh).and_then(|m| m.compute_aabb()) else {
                    return;
                };
                min = min.min(Vec3::from(aabb.min()));
                max = max.max(Vec3::from(aabb.max()));
            }
            if min.x > max.x {
                return;
            }
            ((min + max) / 2.0, (max - min).length() / 2.0)
        }
        Mode::Models => {
            let Some(name) = catalogue.models.get(selection.file) else {
                return;
            };
            let path = format!("{ORIGINAL_SOURCE}://Models/{name}.3dmf");
            let Some(model) = assets
                .get_handle::<Model>(&path)
                .and_then(|h| models.get(&h))
            else {
                return;
            };
            let Some(group) = model.groups.get(selection.item % model.groups.len().max(1)) else {
                return;
            };
            (Vec3::from(group.aabb.center), group.radius.max(1.0))
        }
    };
    orbit.target = center;
    orbit.distance = radius * 2.5;
    orbit.framed = true;
}

fn orbit_camera(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut cameras: Query<(&mut Orbit, &mut Transform)>,
) {
    const TURN_RATE: f32 = 1.5; // radians per second
    const ZOOM_PER_LINE: f32 = 0.1;
    let dt = time.delta_secs();
    for (mut orbit, mut transform) in &mut cameras {
        let axis = |neg: KeyCode, pos: KeyCode| {
            f32::from(u8::from(keys.pressed(pos))) - f32::from(u8::from(keys.pressed(neg)))
        };
        orbit.yaw += axis(KeyCode::ArrowLeft, KeyCode::ArrowRight) * TURN_RATE * dt;
        orbit.pitch = (orbit.pitch + axis(KeyCode::ArrowDown, KeyCode::ArrowUp) * TURN_RATE * dt)
            .clamp(-1.5, 1.5);
        orbit.distance *= 1.0 - scroll.delta.y.clamp(-5.0, 5.0) * ZOOM_PER_LINE;

        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        transform.translation = orbit.target + rotation * Vec3::new(0.0, 0.0, orbit.distance);
        transform.look_at(orbit.target, Vec3::Y);
    }
}

fn update_label(
    selection: Res<Selection>,
    catalogue: Res<Catalogue>,
    assets: Res<AssetServer>,
    models: Res<Assets<Model>>,
    subjects: Query<
        (
            Option<&SkeletonRig>,
            Option<&SkeletonAnimator>,
            Option<&AnimationFlags>,
        ),
        With<Subject>,
    >,
    mut labels: Query<&mut Text, With<Label>>,
) {
    let Ok(mut text) = labels.single_mut() else {
        return;
    };
    let mut lines = Vec::new();
    match selection.mode {
        Mode::Skeletons => {
            let name = catalogue
                .skeletons
                .get(selection.file)
                .map_or("-", String::as_str);
            lines.push(format!(
                "Skeleton {}/{}: {name}",
                selection.file + 1,
                catalogue.skeletons.len()
            ));
            if let Ok((Some(rig), Some(animator), flags)) = subjects.single() {
                let anims = &rig.definition.animations;
                let anim_name = anims.get(animator.anim).map_or("-", |a| a.name.as_str());
                lines.push(format!(
                    "Animation {}/{}: {anim_name}",
                    animator.anim + 1,
                    anims.len()
                ));
                lines.push(format!(
                    "Tick {:.1}{}{}",
                    animator.time,
                    if animator.has_stopped {
                        "  stopped"
                    } else {
                        ""
                    },
                    if animator.is_morphing() {
                        "  morphing"
                    } else {
                        ""
                    },
                ));
                if let Some(flags) = flags {
                    lines.push(format!("Flags {:?}", flags.0));
                }
                lines.push(format!("Bones {}", rig.definition.bones.len()));
            } else {
                lines.push("Loading...".into());
            }
        }
        Mode::Models => {
            let name = catalogue
                .models
                .get(selection.file)
                .map_or("-", String::as_str);
            lines.push(format!(
                "Model file {}/{}: {name}",
                selection.file + 1,
                catalogue.models.len()
            ));
            let path = format!("{ORIGINAL_SOURCE}://Models/{name}.3dmf");
            match assets
                .get_handle::<Model>(&path)
                .and_then(|h| models.get(&h))
            {
                Some(model) => {
                    let count = model.groups.len().max(1);
                    let index = selection.item % count;
                    let parts = model.groups.get(index).map_or(0, |g| g.parts.len());
                    lines.push(format!("Object {index} of {count} ({parts} meshes)"));
                }
                None => lines.push("Loading...".into()),
            }
        }
    }
    if selection.paused {
        lines.push("Paused".into());
    }
    lines.push(String::new());
    lines.push("Space mode | Tab file | Enter item | arrows/wheel camera | P pause".into());
    text.0 = lines.join("\n");
}

/// Screenshot mode for automated checks (see the module docs).
mod capture {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};

    use super::*;

    #[derive(Resource)]
    pub struct Capture {
        path: String,
        /// Real seconds to wait after the camera is framed (and the animation
        /// frozen, if requested), so rendering has settled.
        delay: f32,
        freeze_at_tick: Option<f32>,
        state: State,
    }

    enum State {
        Waiting,
        Taken { frames: u32 },
    }

    pub fn setup(
        mut commands: Commands,
        catalogue: Res<Catalogue>,
        mut selection: ResMut<Selection>,
    ) {
        let Ok(spec) = std::env::var("BUGDOM_VIEWER_CAPTURE") else {
            return;
        };
        let parts: Vec<&str> = spec.splitn(4, ':').collect();
        let [kind, name, item, path] = parts[..] else {
            error!("BUGDOM_VIEWER_CAPTURE must be <kind>:<name>:<item>:<png>");
            return;
        };
        let (mode, list) = match kind {
            "skeleton" => (Mode::Skeletons, &catalogue.skeletons),
            _ => (Mode::Models, &catalogue.models),
        };
        let Some(file) = list.iter().position(|n| n == name) else {
            error!("no {kind} named {name}");
            return;
        };
        *selection = Selection {
            mode,
            file,
            item: item.parse().unwrap_or(0),
            paused: false,
        };
        commands.insert_resource(Capture {
            path: path.to_owned(),
            delay: 1.0,
            freeze_at_tick: std::env::var("BUGDOM_VIEWER_CAPTURE_TICK")
                .ok()
                .and_then(|t| t.parse().ok()),
            state: State::Waiting,
        });
    }

    pub fn run(
        mut commands: Commands,
        time: Res<Time<Real>>,
        capture: Option<ResMut<Capture>>,
        orbits: Query<&Orbit>,
        animators: Query<&SkeletonAnimator>,
        mut virtual_time: ResMut<Time<Virtual>>,
        mut exit: MessageWriter<AppExit>,
    ) {
        let Some(mut capture) = capture else { return };
        if !orbits.iter().any(|o| o.framed) {
            return;
        }
        if let Some(tick) = capture.freeze_at_tick {
            // The fixed timestep makes the frozen tick land within one step.
            if !virtual_time.is_paused() {
                if animators.iter().any(|a| a.time >= tick) {
                    virtual_time.pause();
                } else {
                    return;
                }
            }
        }
        match &mut capture.state {
            State::Waiting => {
                capture.delay -= time.delta_secs();
                if capture.delay <= 0.0 {
                    commands
                        .spawn(Screenshot::primary_window())
                        .observe(save_to_disk(capture.path.clone()));
                    capture.state = State::Taken { frames: 0 };
                }
            }
            // Saving happens asynchronously a few frames later.
            State::Taken { frames } => {
                *frames += 1;
                if *frames > 30 {
                    exit.write(AppExit::Success);
                }
            }
        }
    }
}
