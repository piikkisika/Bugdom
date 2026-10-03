//! The ant pipes, which leak water once their valve opens.
//!
//! Port of `AddBentAntPipe`, `AddHorizAntPipe` and `MoveAntPipe`
//! (original/src/Items/Items.c).

use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::{anthill_models, emit_burst, valve_is_open};
use crate::collision::{CollisionBox, CollisionKind};
use crate::effects::{
    EffectsSystems, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture,
};
use crate::items::kind as item;
use crate::items::scenery::StaticObject;
use crate::items::{ItemSpawn, RegisterItemKind};
use crate::liquids::WaterValves;
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::player::PlayerSystems;
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::BENT_ANT_PIPE, add_bent_ant_pipe)
        .register_item_kind(item::HORIZ_ANT_PIPE, add_horiz_ant_pipe)
        .add_systems(
            FixedUpdate,
            leak_ant_pipes
                .after(PlayerSystems::Move)
                .before(EffectsSystems::MoveParticles)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `BENT_PIPE_SCALE` and `HORIZ_PIPE_SCALE`.
const BENT_PIPE_SCALE: f32 = 1.0;
const HORIZ_PIPE_SCALE: f32 = 0.5;
/// Where each pipe leaks, before its turn and scale.
const BENT_PIPE_LEAK: Vec3 = Vec3::new(45.0, 178.0, 45.0);
const HORIZ_PIPE_LEAK: Vec3 = Vec3::new(-58.0, 400.0, 214.0);
/// How high a horizontal pipe sits per step of its `params[1]`, in units.
const HORIZ_PIPE_HEIGHT_STEP: f32 = 10.0;
const BENT_PIPE_BOX: CollisionBox = CollisionBox::new(300.0, 0.0, -50.0, 50.0, 50.0, -50.0);
/// A horizontal pipe's box, running along x, or along z when it is turned
/// an odd number of quarter turns.
const HORIZ_PIPE_BOX_X: CollisionBox = CollisionBox::new(300.0, 0.0, -500.0, 500.0, 100.0, -100.0);
const HORIZ_PIPE_BOX_Z: CollisionBox = CollisionBox::new(300.0, 0.0, -100.0, 100.0, 500.0, -500.0);

/// Seconds between bursts of water.
const LEAK_INTERVAL: f32 = 0.05;
const LEAK_DROPS_PER_BURST: usize = 3;
/// The drops' random sideways speed (the whole width), in units per second.
const LEAK_SPEED: f32 = 150.0;
const LEAK_DROP_SCALE: f32 = 1.0;
const LEAK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::BOUNCE,
    gravity: 700.0,
    magnetism: 0.0,
    base_scale: 25.0,
    decay_rate: -1.2,
    fade_rate: 0.6,
    texture: ParticleTexture::Patchy,
};

/// An ant pipe, which can leak water.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AntPipe {
    /// The valve that starts it leaking (`ValveID`), when it has one
    /// (`ValvePipe`).
    pub valve: Option<u8>,
    /// Whether it is leaking (`SpewWater`).
    pub leaking: bool,
    /// Where the water comes out, in the world.
    leak_point: Vec3,
    /// Seconds since the last burst (`SpewWaterTimer`).
    leak_timer: f32,
    drops: Option<ParticleGroupId>,
}

impl AntPipe {
    /// A pipe at `position`, turned by `yaw`, from its valve id and its
    /// item's `params[3]`: bit 0 makes it leak when the valve opens and
    /// bit 1 makes it always leak.
    fn new(valve: u8, flags: u8, position: Vec3, yaw: f32, leak_offset: Vec3) -> Self {
        Self {
            valve: (flags & 1 != 0).then_some(valve),
            leaking: flags & (1 << 1) != 0,
            leak_point: position + Quat::from_rotation_y(yaw) * leak_offset,
            leak_timer: 0.0,
            drops: None,
        }
    }

    /// Counts up to the next burst; true when one is due.
    fn drops_due(&mut self, dt: f32) -> bool {
        self.leak_timer += dt;
        if self.leak_timer > LEAK_INTERVAL {
            self.leak_timer = 0.0;
            true
        } else {
            false
        }
    }
}

fn pipe_kinds() -> LayerMask {
    LayerMask::from([CollisionKind::Misc, CollisionKind::BlockCamera])
}

/// Port of `AddBentAntPipe`. `params[0]` is its turn in quarter turns,
/// `params[1]` its valve and `params[3]` how it leaks (see [`AntPipe`]).
fn add_bent_ant_pipe(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
) -> bool {
    let yaw = f32::from(spawn.params[0]) * FRAC_PI_2;
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    let pipe = StaticObject {
        y,
        model: ModelRef::new(ModelFile::Level1, anthill_models::BENT_PIPE),
        shading: Shading::Lit,
        yaw,
        scale: BENT_PIPE_SCALE,
        boxes: vec![BENT_PIPE_BOX],
        kinds: pipe_kinds(),
    }
    .spawn("Bent ant pipe", &mut commands, &mut models, &spawn);
    let position = Vec3::new(spawn.position.x, y, spawn.position.y);
    commands.entity(pipe).insert(AntPipe::new(
        spawn.params[1],
        spawn.params[3],
        position,
        yaw,
        BENT_PIPE_LEAK * BENT_PIPE_SCALE,
    ));
    true
}

/// Port of `AddHorizAntPipe`. `params[0]` is its turn in quarter turns,
/// `params[1]` its height above the floor in steps of ten units,
/// `params[2]` its valve and `params[3]` how it leaks (see [`AntPipe`]).
fn add_horiz_ant_pipe(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
) -> bool {
    let yaw = f32::from(spawn.params[0]) * FRAC_PI_2;
    let y = map.floor_height(spawn.position.x, spawn.position.y)
        + f32::from(spawn.params[1]) * HORIZ_PIPE_HEIGHT_STEP;
    let pipe_box = if spawn.params[0] & 1 != 0 {
        HORIZ_PIPE_BOX_Z
    } else {
        HORIZ_PIPE_BOX_X
    };
    let pipe = StaticObject {
        y,
        model: ModelRef::new(ModelFile::Level1, anthill_models::HORIZ_PIPE),
        shading: Shading::Lit,
        yaw,
        scale: HORIZ_PIPE_SCALE,
        boxes: vec![pipe_box],
        kinds: pipe_kinds(),
    }
    .spawn("Horizontal ant pipe", &mut commands, &mut models, &spawn);
    let position = Vec3::new(spawn.position.x, y, spawn.position.y);
    commands.entity(pipe).insert(AntPipe::new(
        spawn.params[2],
        spawn.params[3],
        position,
        yaw,
        HORIZ_PIPE_LEAK * HORIZ_PIPE_SCALE,
    ));
    true
}

/// Starts the pipes whose valve has opened leaking, and drips their water.
/// Port of `MoveAntPipe`; its `TrackTerrainItem` is the pipe's
/// `DespawnOutOfRange`.
fn leak_ant_pipes(
    time: Res<Time>,
    valves: Res<WaterValves>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut pipes: Query<&mut AntPipe>,
) {
    for mut pipe in &mut pipes {
        if pipe.valve.is_some_and(|id| valve_is_open(&valves, id)) {
            pipe.leaking = true;
        }
        if !pipe.leaking {
            // Sound: stop EFFECT_WATERLEAK.
            continue;
        }
        // Sound: EFFECT_WATERLEAK at the leak, started or kept up.
        if !pipe.drops_due(time.delta_secs()) {
            continue;
        }
        let at = pipe.leak_point;
        emit_burst(
            &mut groups,
            &mut random,
            &mut pipe.drops,
            LEAK_GROUP,
            LEAK_DROPS_PER_BURST,
            false,
            |random, _| {
                let x = (random.next_f32() - 0.5) * LEAK_SPEED;
                let z = (random.next_f32() - 0.5) * LEAK_SPEED;
                (at, Vec3::new(x, 0.0, z), LEAK_DROP_SCALE)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn the_leak_turns_with_the_pipe() {
        let pipe = AntPipe::new(0, 0, Vec3::new(100.0, 10.0, 0.0), FRAC_PI_2, BENT_PIPE_LEAK);
        // A quarter turn takes +x to −z and +z to +x.
        assert!(
            pipe.leak_point
                .abs_diff_eq(Vec3::new(145.0, 188.0, -45.0), 1e-3)
        );
    }

    #[test]
    fn the_flags_choose_how_a_pipe_leaks() {
        let valve_pipe = AntPipe::new(4, 1, Vec3::ZERO, 0.0, Vec3::ZERO);
        assert_eq!(valve_pipe.valve, Some(4));
        assert!(!valve_pipe.leaking);
        let always = AntPipe::new(4, 2, Vec3::ZERO, 0.0, Vec3::ZERO);
        assert_eq!(always.valve, None);
        assert!(always.leaking);
    }

    #[test]
    fn a_pipe_leaks_once_its_valve_opens() {
        let mut world = World::new();
        world.init_resource::<WaterValves>();
        world.init_resource::<ParticleGroups>();
        world.init_resource::<GameRandom>();
        world.insert_resource(Time::<()>::default());
        let pipe = world
            .spawn(AntPipe::new(4, 1, Vec3::ZERO, 0.0, Vec3::ZERO))
            .id();
        let tick = |world: &mut World| {
            world
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(60));
            world.run_system_once(leak_ant_pipes).unwrap();
        };
        tick(&mut world);
        assert!(!world.get::<AntPipe>(pipe).unwrap().leaking);
        assert_eq!(world.resource::<ParticleGroups>().iter().count(), 0);
        world.resource_mut::<WaterValves>().0[4] = true;
        tick(&mut world);
        let leaking = world.get::<AntPipe>(pipe).unwrap();
        assert!(leaking.leaking);
        let group = leaking.drops.unwrap();
        let groups = world.resource::<ParticleGroups>();
        assert_eq!(
            groups.get(group).unwrap().particles().len(),
            LEAK_DROPS_PER_BURST
        );
    }
}
