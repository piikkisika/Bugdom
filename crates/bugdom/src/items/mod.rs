//! Terrain items: the objects placed on the map, which come and go with an
//! active window around the camera.
//!
//! Port of the item parts of original/src/Terrain/Terrain.c and
//! original/src/Terrain/Terrain2.c. The terrain itself is drawn whole, but
//! items still stream in and out exactly as the original's scrolling did,
//! because gameplay depends on it: enemies come back when you return, and
//! only nearby ones are active (docs/design/phase2-engine-core.md §3).
//!
//! Each item kind has a spawn system, registered with
//! [`RegisterItemKind::register_item_kind`] (the original's
//! `gTerrainItemAddRoutines`).

mod anthill;
mod hive;
pub mod kind;
mod pickups;
mod scenery;
mod traps;
mod triggers;

use bevy::ecs::system::SystemId;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bugdom_formats::terrain::Item;

pub use triggers::AreaCompleted;

use crate::assets::terrain::TerrainAsset;
use crate::camera::{CameraSystems, FlyCamera, FollowCamera, GameCamera};
use crate::collision::CollisionSystems;
use crate::level::CurrentLevel;
use crate::player::PlayerSystems;
use crate::state::{AppState, LevelAssets};
use crate::terrain::{MAP_TO_WORLD, PlayerStart, SUPERTILE_TILES, TILE_SIZE, TerrainMap};

pub struct ItemsPlugin;

impl Plugin for ItemsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemKinds>()
            .add_observer(release_item)
            .add_systems(
                OnEnter(AppState::InGame),
                prime_items
                    .in_set(ItemSystems::Window)
                    .after(PlayerSystems::Spawn),
            )
            .add_systems(
                FixedUpdate,
                (
                    despawn_out_of_range
                        .in_set(ItemSystems::Track)
                        .before(CollisionSystems::Gather)
                        .before(PlayerSystems::Move),
                    update_item_window
                        .in_set(ItemSystems::Window)
                        .after(CameraSystems::Follow),
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_plugins((
                anthill::plugin,
                hive::plugin,
                pickups::plugin,
                scenery::plugin,
                traps::plugin,
                triggers::plugin,
            ));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemSystems {
    /// Despawns items that left the window, before anything moves (each
    /// item's `TrackTerrainItem` check in `MoveObjects`).
    Track,
    /// Moves the window after the camera and spawns the items that came
    /// into it (`DoMyTerrainUpdate`).
    Window,
}

/// Size of a supertile in world units (`TERRAIN_SUPERTILE_UNIT_SIZE`).
const SUPERTILE_SIZE: f32 = SUPERTILE_TILES as f32 * TILE_SIZE;
/// Size of a supertile in map pixels, the units items are stored in.
const SUPERTILE_MAP_SIZE: i32 = (SUPERTILE_SIZE / MAP_TO_WORLD) as i32;
/// How many supertiles beyond the terrain window items are added
/// (`ITEM_WINDOW`).
const ITEM_WINDOW: i32 = 1;
/// How far beyond the add window items are kept, in supertiles
/// (`OUTER_SIZE`).
const OUTER_SIZE: f32 = 0.6;
/// The window follows this point in front of the camera, in world units.
const LOOK_AHEAD: f32 = 500.0;

/// What a spawn system is given for one item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemSpawn {
    /// The item's index in [`TerrainItems`]; put it in a
    /// [`TerrainItemSource`] on the spawned entity.
    pub index: u32,
    pub kind: u16,
    /// World x and z.
    pub position: Vec2,
    pub params: [u8; 4],
    /// The item's flags, which persist while the level runs (e.g. a door
    /// that has been opened).
    pub flags: u16,
}

/// The spawn system of each item kind (`gTerrainItemAddRoutines`). A spawn
/// system returns whether it spawned something; only then is the item
/// marked as in use.
#[derive(Resource, Debug, Default)]
pub struct ItemKinds(HashMap<u16, SystemId<In<ItemSpawn>, bool>>);

pub trait RegisterItemKind {
    /// Makes `system` spawn items of `kind`.
    fn register_item_kind<M>(
        &mut self,
        kind: u16,
        system: impl IntoSystem<In<ItemSpawn>, bool, M> + 'static,
    ) -> &mut Self;
}

impl RegisterItemKind for App {
    fn register_item_kind<M>(
        &mut self,
        kind: u16,
        system: impl IntoSystem<In<ItemSpawn>, bool, M> + 'static,
    ) -> &mut Self {
        let id = self.world_mut().register_system(system);
        self.world_mut()
            .resource_mut::<ItemKinds>()
            .0
            .insert(kind, id);
        self
    }
}

/// The level's items and which of them are spawned (`gMasterItemList` and
/// `ITEM_FLAGS_INUSE`).
#[derive(Resource, Debug, Clone, Default)]
pub struct TerrainItems {
    pub items: Vec<Item>,
    in_use: Vec<bool>,
}

impl TerrainItems {
    pub fn new(items: Vec<Item>) -> Self {
        let in_use = vec![false; items.len()];
        Self { items, in_use }
    }

    /// Marks an item as spawned, so that it isn't spawned again. An entity
    /// that drops its [`TerrainItemSource`] and then calls this keeps its
    /// item from ever coming back (`TerrainItemPtr = nil`).
    pub fn mark_in_use(&mut self, index: u32) {
        if let Some(in_use) = self.in_use.get_mut(index as usize) {
            *in_use = true;
        }
    }

    /// Sets bits in an item's flags, which outlive its entity.
    pub fn set_flags(&mut self, index: u32, flags: u16) {
        if let Some(item) = self.items.get_mut(index as usize) {
            item.flags |= flags;
        }
    }
}

/// The map item an entity was spawned from (`TerrainItemPtr`). Despawning
/// the entity makes the item available again.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainItemSource(pub u32);

/// Keeps an entity's map item from ever spawning again: the entity drops
/// its [`TerrainItemSource`], and the item stays marked as in use
/// (`theNode->TerrainItemPtr = nil`, as killed enemies and used-up items
/// do).
pub fn forget_terrain_item(entity: &mut EntityCommands) {
    entity.queue(|mut entity: EntityWorldMut| {
        let Some(TerrainItemSource(index)) = entity.take::<TerrainItemSource>() else {
            return;
        };
        entity.world_scope(|world| {
            if let Some(mut items) = world.get_resource_mut::<TerrainItems>() {
                items.mark_in_use(index);
            }
        });
    });
}

/// Despawns the entity once it leaves the item window (`TrackTerrainItem`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DespawnOutOfRange;

/// The supertile window items live in.
///
/// The original scrolls its terrain cache one supertile at a time and adds
/// the items in each row or column that comes into range. `row` and `col`
/// are the window's far-left supertile (`gCurrentSuperTileRow`/`Col`).
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct ItemWindow {
    pub row: i32,
    pub col: i32,
    /// The window's size in supertiles (`SUPERTILE_DIST_WIDE`/`DEEP`).
    pub size: i32,
    supertiles_wide: i32,
    supertiles_deep: i32,
    /// Items outside this, in world x and z, are deleted
    /// (`gTerrainItemDeleteWindow_*`).
    pub delete_min: Vec2,
    pub delete_max: Vec2,
}

impl ItemWindow {
    fn new(map: &TerrainMap, active_range: u8, focus: Vec2) -> Self {
        let (wide, deep) = map.supertiles();
        let size = i32::from(active_range) * 2;
        let (col, row) =
            supertile_of(focus - Vec2::splat(f32::from(active_range) * SUPERTILE_SIZE));
        let mut window = Self {
            row,
            col,
            size,
            supertiles_wide: wide as i32,
            supertiles_deep: deep as i32,
            delete_min: Vec2::ZERO,
            delete_max: Vec2::ZERO,
        };
        window.update_delete_window();
        window
    }

    /// Whether a world point is on a supertile inside the window, i.e. one
    /// the original would have built (`gTerrainScrollBuffer`).
    pub fn contains_point(&self, point: Vec2) -> bool {
        let (col, row) = supertile_of(point);
        (self.row..self.row + self.size).contains(&row)
            && (self.col..self.col + self.size).contains(&col)
            && (0..self.supertiles_deep).contains(&row)
            && (0..self.supertiles_wide).contains(&col)
    }

    /// Whether an item at this world position should be deleted
    /// (`IsPositionOutOfRange`).
    pub fn is_out_of_range(&self, position: Vec2) -> bool {
        position.x < self.delete_min.x
            || position.x > self.delete_max.x
            || position.y < self.delete_min.y
            || position.y > self.delete_max.y
    }

    /// Port of `CalcNewItemDeleteWindow` (original/src/Terrain/Terrain.c).
    fn update_delete_window(&mut self) {
        let border = (ITEM_WINDOW as f32 + OUTER_SIZE) * SUPERTILE_SIZE;
        let corner = Vec2::new(self.col as f32, self.row as f32) * SUPERTILE_SIZE;
        self.delete_min = corner - Vec2::splat(border);
        self.delete_max = corner + Vec2::splat(self.size as f32 * SUPERTILE_SIZE + border);
    }

    /// Moves the window to the supertile `target` (far-left corner) and
    /// returns the supertile ranges to scan for new items, as
    /// `(top, bottom, left, right)`.
    ///
    /// Port of `DoMyTerrainUpdate` and the item parts of `ScrollTerrainUp`,
    /// `Down`, `Left` and `Right`. The original can only scroll one
    /// supertile per frame and stops the game otherwise; here bigger jumps
    /// are taken one supertile at a time.
    fn scroll_to(&mut self, target_col: i32, target_row: i32) -> Vec<[i32; 4]> {
        let mut scans = Vec::new();
        // Vertical first, as the original does.
        while target_row != self.row {
            let row = self.row + (target_row - self.row).signum();
            let scan = if row > self.row {
                self.scan_down_edge(row, target_col)
            } else {
                self.scan_up_edge(row, target_col)
            };
            scans.extend(scan);
            self.row = row;
        }
        while target_col != self.col {
            if target_col > self.col {
                match self.scan_right_edge() {
                    ScrollRight::Scan(scan) => scans.push(scan),
                    ScrollRight::Skip => {}
                    // The original returns before moving the window here.
                    ScrollRight::Stuck => break,
                }
                self.col += 1;
            } else {
                let col = self.col - 1;
                scans.extend(self.scan_left_edge(col, self.row));
                self.col = col;
            }
        }
        self.update_delete_window();
        scans
    }

    /// The row entering at the near (+Z) side when the window moves to
    /// `row` (`ScrollTerrainUp`).
    fn scan_down_edge(&self, row: i32, col: i32) -> Option<[i32; 4]> {
        let terrain_row = row + self.size - 1;
        if !(0..self.supertiles_deep).contains(&terrain_row) {
            return None;
        }
        let bottom = terrain_row + ITEM_WINDOW;
        let (left, right) = self.clamp_columns(col)?;
        (0..self.supertiles_deep)
            .contains(&bottom)
            .then_some([bottom, bottom, left, right])
    }

    /// The row entering at the far (−Z) side (`ScrollTerrainDown`).
    fn scan_up_edge(&self, row: i32, col: i32) -> Option<[i32; 4]> {
        if !(0..self.supertiles_deep).contains(&row) {
            return None;
        }
        let top = row - ITEM_WINDOW;
        let (left, right) = self.clamp_columns(col)?;
        (0..self.supertiles_deep)
            .contains(&top)
            .then_some([top, top, left, right])
    }

    /// The columns a new row covers, clipped to the map, or `None` if it is
    /// entirely off it.
    fn clamp_columns(&self, col: i32) -> Option<(i32, i32)> {
        let left = col - ITEM_WINDOW;
        let right = col + self.size - 1 + ITEM_WINDOW;
        if left >= self.supertiles_wide || right < 0 {
            return None;
        }
        Some((left.max(0), right.min(self.supertiles_wide - 1)))
    }

    /// The column entering at the +X side when the window moves one
    /// supertile right (`ScrollTerrainLeft`).
    fn scan_right_edge(&self) -> ScrollRight {
        let terrain_col = self.col + self.size;
        if !(0..self.supertiles_wide).contains(&terrain_col) {
            return ScrollRight::Skip;
        }
        let right = terrain_col + ITEM_WINDOW;
        if !(0..self.supertiles_wide).contains(&right) {
            return ScrollRight::Skip;
        }
        let top = self.row - ITEM_WINDOW;
        let bottom = self.row + self.size - 1 + ITEM_WINDOW;
        if top >= self.supertiles_deep {
            return ScrollRight::Stuck;
        }
        if bottom < 0 {
            return ScrollRight::Skip;
        }
        ScrollRight::Scan([top.max(0), bottom, right, right])
    }

    /// The column entering at the −X side when the window moves to `col`
    /// (`ScrollTerrainRight`).
    fn scan_left_edge(&self, col: i32, row: i32) -> Option<[i32; 4]> {
        if !(0..self.supertiles_wide).contains(&col) {
            return None;
        }
        let left = col - ITEM_WINDOW;
        let top = row - ITEM_WINDOW;
        let bottom = row + self.size - 1 + ITEM_WINDOW;
        if !(0..self.supertiles_wide).contains(&left) || top >= self.supertiles_deep || bottom < 0 {
            return None;
        }
        Some([top.max(0), bottom, left, left])
    }
}

enum ScrollRight {
    Scan([i32; 4]),
    Skip,
    Stuck,
}

/// The supertile column and row of a world point, truncated toward zero as
/// `GetSuperTileInfo` does.
fn supertile_of(point: Vec2) -> (i32, i32) {
    let col = (point.x as i32) as f32 * (1.0 / SUPERTILE_SIZE);
    let row = (point.y as i32) as f32 * (1.0 / SUPERTILE_SIZE);
    (col as i32, row as i32)
}

/// The free items inside the supertile range `[top, bottom, left, right]`.
/// Port of the selection in `ScanForPlayfieldItems`.
fn items_in(items: &TerrainItems, [top, bottom, left, right]: [i32; 4]) -> Vec<u32> {
    let min_x = left * SUPERTILE_MAP_SIZE;
    let max_x = right * SUPERTILE_MAP_SIZE + SUPERTILE_MAP_SIZE - 1;
    let min_z = top * SUPERTILE_MAP_SIZE;
    let max_z = bottom * SUPERTILE_MAP_SIZE + SUPERTILE_MAP_SIZE - 1;
    items
        .items
        .iter()
        .zip(&items.in_use)
        .enumerate()
        .filter(|(_, (item, in_use))| {
            !**in_use
                && (min_x..=max_x).contains(&i32::from(item.x))
                && (min_z..=max_z).contains(&i32::from(item.z))
        })
        .map(|(index, _)| index as u32)
        .collect()
}

/// Runs the spawn systems for the items in the given ranges.
/// Port of `ScanForPlayfieldItems` (original/src/Terrain/Terrain2.c).
fn spawn_items(world: &mut World, scans: &[[i32; 4]]) {
    for &scan in scans {
        let indices = items_in(world.resource::<TerrainItems>(), scan);
        for index in indices {
            let item = world.resource::<TerrainItems>().items[index as usize];
            let Some(&system) = world.resource::<ItemKinds>().0.get(&item.kind) else {
                // Kinds without a spawn system yet, and the original's
                // `NilAdd` kinds such as the start point.
                continue;
            };
            let spawn = ItemSpawn {
                index,
                kind: item.kind,
                position: Vec2::new(f32::from(item.x), f32::from(item.z)) * MAP_TO_WORLD,
                params: item.params,
                flags: item.flags,
            };
            match world.run_system_with(system, spawn) {
                Ok(true) => world.resource_mut::<TerrainItems>().in_use[index as usize] = true,
                Ok(false) => {}
                Err(error) => error!("Item kind {} failed to spawn: {error}", item.kind),
            }
        }
    }
}

/// Sets up the items and the window when the level starts, and adds the
/// items around the start.
///
/// Port of `InitCurrentScrollSettings` and `PrimeInitialTerrain`
/// (original/src/Terrain/Terrain.c). The window starts around the player's
/// start, not the camera.
fn prime_items(world: &mut World) {
    let level_assets = world.resource::<LevelAssets>().clone();
    let Some(terrain) = world
        .resource::<Assets<TerrainAsset>>()
        .get(&level_assets.terrain)
    else {
        return;
    };
    let items = TerrainItems::new(terrain.items.clone());
    let range = world
        .resource::<CurrentLevel>()
        .def()
        .settings()
        .supertile_active_range;
    let start = world.resource::<PlayerStart>().position;
    let mut window = ItemWindow::new(world.resource::<TerrainMap>(), range, start);

    // Start one window's width (plus the item border) to the left and
    // scroll into place, adding the items column by column.
    let steps = window.size + ITEM_WINDOW + 1;
    window.col -= steps;
    let target = window.col + steps;
    let scans = window.scroll_to(target, window.row);

    world.insert_resource(items);
    world.insert_resource(window);
    spawn_items(world, &scans);
}

/// Moves the window to follow the point in front of the camera and adds
/// the items that come into it.
/// Port of `DoMyTerrainUpdate` (original/src/Terrain/Terrain.c).
fn update_item_window(world: &mut World) {
    let mut cameras =
        world.query_filtered::<(&FollowCamera, &Transform, Has<FlyCamera>), With<GameCamera>>();
    let Some((from, look)) = cameras
        .iter(world)
        .next()
        .map(|(follow, transform, flying)| {
            if flying {
                // Debugging: stream around wherever the fly camera looks.
                (transform.translation.xz(), transform.forward().xz())
            } else {
                (follow.from.xz(), follow.to.xz() - follow.from.xz())
            }
        })
    else {
        return;
    };
    let focus = from + look.normalize_or_zero() * LOOK_AHEAD;
    let Some(mut window) = world.get_resource_mut::<ItemWindow>() else {
        return;
    };
    let range = window.size as f32 / 2.0;
    let (col, row) = supertile_of(focus - Vec2::splat(range * SUPERTILE_SIZE));
    if col == window.col && row == window.row {
        return;
    }
    let scans = window.scroll_to(col, row);
    spawn_items(world, &scans);
}

/// Port of `TrackTerrainItem` as the items' move functions use it.
fn despawn_out_of_range(
    mut commands: Commands,
    window: Res<ItemWindow>,
    items: Query<(Entity, &Transform), With<DespawnOutOfRange>>,
) {
    for (entity, transform) in &items {
        if window.is_out_of_range(transform.translation.xz()) {
            commands.entity(entity).despawn();
        }
    }
}

/// Makes an item available again when its entity goes (the end of
/// `DeleteObject`).
fn release_item(
    remove: On<Remove, TerrainItemSource>,
    sources: Query<&TerrainItemSource>,
    items: Option<ResMut<TerrainItems>>,
) {
    let (Ok(source), Some(mut items)) = (sources.get(remove.entity), items) else {
        return;
    };
    if let Some(in_use) = items.in_use.get_mut(source.0 as usize) {
        *in_use = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> ItemWindow {
        let map = TerrainMap::load_for_tests("Lawn", false);
        ItemWindow::new(&map, 5, Vec2::new(12720.0, 15780.0))
    }

    #[test]
    fn a_forgotten_item_stays_in_use() {
        let item = Item {
            x: 0,
            z: 0,
            kind: 0,
            params: [0; 4],
            flags: 0,
        };
        let mut world = World::new();
        world.add_observer(release_item);
        let mut items = TerrainItems::new(vec![item; 2]);
        items.in_use = vec![true, true];
        world.insert_resource(items);
        let kept = world.spawn(TerrainItemSource(0)).id();
        let freed = world.spawn(TerrainItemSource(1)).id();
        forget_terrain_item(&mut world.commands().entity(kept));
        world.flush();
        assert!(!world.entity(kept).contains::<TerrainItemSource>());
        world.despawn(kept);
        world.despawn(freed);
        assert_eq!(world.resource::<TerrainItems>().in_use, [true, false]);
    }

    #[test]
    fn priming_scans_the_whole_add_window() {
        let mut window = window();
        let (col, row) = (window.col, window.row);
        let steps = window.size + ITEM_WINDOW + 1;
        window.col -= steps;
        let scans = window.scroll_to(col, row);
        assert_eq!(window.col, col);
        // One column per step, from the window's left border to its right.
        let columns: Vec<i32> = scans.iter().map(|s| s[2]).collect();
        // A column is only scanned when the terrain column beside it is on
        // the map too.
        let on_map = |c: i32| (0..window.supertiles_wide).contains(&c);
        let expected: Vec<i32> = (col - 1..=col + window.size)
            .filter(|&c| on_map(c) && on_map(c - 1))
            .collect();
        assert_eq!(columns, expected);
        for scan in &scans {
            assert_eq!(scan[0], (row - 1).max(0));
            assert_eq!(scan[1], row + window.size);
        }
    }

    #[test]
    fn moving_one_supertile_scans_the_new_edge() {
        let mut window = window();
        let (col, row) = (window.col, window.row);
        let scans = window.scroll_to(col, row + 1);
        assert_eq!(
            scans,
            [[
                row + 1 + window.size,
                row + 1 + window.size,
                col - 1,
                col + window.size
            ]]
        );
        let scans = window.scroll_to(col - 1, row + 1);
        assert_eq!(scans, [[row, row + 1 + window.size, col - 2, col - 2]]);
    }

    #[test]
    fn the_delete_window_is_bigger_than_the_add_window() {
        let window = window();
        let corner = Vec2::new(window.col as f32, window.row as f32) * SUPERTILE_SIZE;
        // An item one supertile beyond the window was added, and is kept.
        assert!(!window.is_out_of_range(corner - Vec2::splat(SUPERTILE_SIZE * 1.5)));
        assert!(window.is_out_of_range(corner - Vec2::splat(SUPERTILE_SIZE * 1.7)));
    }
}
