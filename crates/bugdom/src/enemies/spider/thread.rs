//! The thread a spider drops on from high up.
//!
//! Port of the thread parts of `AddEnemy_Spider`, `MoveSpider_Drop` and of
//! `MoveThread` (original/src/Enemies/Enemy_Spider.c). The thread is one of
//! the level's models (`FOREST_MObjType_Thread`, `NIGHT_MObjType_Thread`).
//! While the spider drops it is the spider's `ChainNode` ([`ThreadOf`]):
//! it hangs above the spider and goes when the spider goes. Once the
//! spider lands, the thread is let go ([`LooseThread`]) and stays where it
//! is until it leaves the item window or the camera's view.

use avian3d::prelude::TransformInterpolation;
use bevy::prelude::*;

use super::{SPIDER_SCALE, SPIDER_START_YOFF, SpiderBrain, web_models};
use crate::enemies::EnemyCulling;
use crate::items::DespawnOutOfRange;
use crate::level::CurrentLevel;
use crate::objects::{ModelSpawner, Shading};
use crate::state::AppState;

/// How far above the spider's origin the thread's origin hangs, in units
/// (`THREAD_YOFF`).
pub const THREAD_YOFF: f32 = 100.0;

/// A spider's thread while the spider hangs on it (`ChainNode`). Despawning
/// the spider despawns it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = SpiderThreads)]
pub struct ThreadOf(pub Entity);

/// The thread a spider hangs on, despawned with the spider.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
#[relationship_target(relationship = ThreadOf, linked_spawn)]
pub struct SpiderThreads(Vec<Entity>);

/// A spider's thread.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct SpiderThread {
    /// Its bounding sphere's radius, scaled, for the cull test.
    pub radius: f32,
}

/// A thread whose spider has landed (`MoveCall = MoveThread`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LooseThread;

/// Gives a new spider its hidden thread. It arrives when the commands are
/// applied, after the spider's [`SpiderBrain`].
pub fn give_thread(commands: &mut Commands, spider: Entity) {
    commands.run_system_cached_with(give_spider_thread, spider);
}

/// Port of the thread made in `AddEnemy_Spider`
/// (original/src/Enemies/Enemy_Spider.c): hidden, [`THREAD_YOFF`] above
/// the floor under the spider, at the spider's scale.
fn give_spider_thread(
    In(spider): In<Entity>,
    mut commands: Commands,
    mut models: ModelSpawner,
    level: Res<CurrentLevel>,
    mut spiders: Query<(&Transform, &mut SpiderBrain)>,
) {
    let Ok((transform, mut brain)) = spiders.get_mut(spider) else {
        return;
    };
    let Some(model) = web_models(level.def().level_type).map(|m| m.thread) else {
        return;
    };
    // `MakeEnemySkeleton` leaves the floor height in the object
    // definition; only the spider is lifted.
    let at = transform.translation - Vec3::Y * (SPIDER_START_YOFF - THREAD_YOFF);
    let radius = models.radius(model).unwrap_or(0.0) * SPIDER_SCALE;
    let thread = commands
        .spawn((
            Name::new("Spider thread"),
            SpiderThread { radius },
            ThreadOf(spider),
            Transform::from_translation(at),
            Visibility::Hidden,
            TransformInterpolation,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        &mut commands,
        thread,
        model,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(SPIDER_SCALE)),
    );
    brain.thread = Some(thread);
}

/// Lets a landed spider's thread go: it now stays where it is and goes on
/// its own (`threadObj->MoveCall = MoveThread`, `ChainNode = nil`).
pub fn let_go_of_thread(commands: &mut Commands, thread: Entity) {
    commands
        .entity(thread)
        .remove::<ThreadOf>()
        .insert((LooseThread, DespawnOutOfRange));
}

/// Deletes the loose threads the camera no longer sees. Leaving the item
/// window (`TrackTerrainItem`) is [`DespawnOutOfRange`].
///
/// Port of `MoveThread` (original/src/Enemies/Enemy_Spider.c). The
/// original reads the cull flag of the last frame drawn; this tests the
/// view as it is now.
pub(super) fn cull_loose_threads(
    mut commands: Commands,
    culling: EnemyCulling,
    threads: Query<(Entity, &Transform, &SpiderThread), With<LooseThread>>,
) {
    for (thread, transform, info) in &threads {
        if culling.is_culled(transform.translation, info.radius) {
            commands.entity(thread).despawn();
        }
    }
}

/// The loose threads' culling, once the objects have moved (`MoveThread`
/// runs in the object list with the spiders). A dropping spider places its
/// own thread, in [`super::move_spiders`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        cull_loose_threads
            .after(crate::enemies::EnemySystems::Move)
            .run_if(in_state(AppState::InGame)),
    );
}
