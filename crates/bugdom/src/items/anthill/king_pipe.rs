//! The king's water pipes: big pipes round the king ant's chamber, which
//! spray water when the ball rams them or the bug kicks them. The spray's
//! particles carry [`ParticleFlags::EXTINGUISH`], which is what makes the
//! king ant wet (`ParticleHitObject` in `MoveKingAnt`); the pipe tells him
//! nothing else.
//!
//! Port of `AddKingWaterPipe`, `MoveKingWaterPipe`, `DoTrig_KingWaterPipe`
//! and `KickKingWaterPipe` (original/src/Items/Triggers2.c).

use avian3d::prelude::LayerMask;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::{anthill_models, emit_burst};
use crate::collision::{CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object};
use crate::effects::{
    EffectsSystems, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture,
};
use crate::items::kind as item;
use crate::items::{ItemSpawn, RegisterItemKind, TerrainItemSource};
use crate::level::CurrentLevel;
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::player::{ItemKicked, Player, PlayerForm, PlayerSpeed, PlayerSystems};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::KING_WATER_PIPE, add_king_water_pipe)
        .add_systems(
            FixedUpdate,
            (hit_king_water_pipes, spray_king_water_pipes)
                .chain()
                .after(PlayerSystems::Move)
                .before(EffectsSystems::MoveParticles)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `LEVEL_NUM_ANTKING`, the only level with these pipes.
const ANT_KING_LEVEL: usize = 9;
const KING_PIPE_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, anthill_models::KING_PIPE);
const KING_PIPE_SCALE: f32 = 4.0;
const KING_PIPE_BOX: CollisionBox = CollisionBox::new(900.0, 0.0, -120.0, 120.0, 120.0, -120.0);
/// How far above the pipe's base the water comes out, in units.
const SPRAY_HEIGHT: f32 = 300.0;
/// How fast the ball must go to set the pipe off, in units per second.
const RAM_SPEED: f32 = 500.0;
/// Seconds a pipe sprays, and seconds it then takes to fill up again.
const SPRAY_TIME: f32 = 4.0;
const REFILL_TIME: f32 = 5.0;
/// Seconds between bursts of spray.
const SPRAY_INTERVAL: f32 = 0.03;
const SPRAY_DROPS_PER_BURST: usize = 8;
/// The drops' random sideways speed (the whole width) and their upward
/// speed, in units per second.
const SPRAY_SIDEWAYS_SPEED: f32 = 950.0;
const SPRAY_RISE: f32 = 30.0;
const SPRAY_DROP_SCALE: f32 = 1.5;
const SPRAY_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::EXTINGUISH.0),
    gravity: 300.0,
    magnetism: 0.0,
    base_scale: 30.0,
    decay_rate: -1.3,
    fade_rate: 0.8,
    texture: ParticleTexture::Patchy,
};

/// A king's water pipe.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct KingWaterPipe {
    /// The pipe's number on the map (`PipeID`); nothing reads it.
    pub id: u8,
    /// Where the water comes out (`InitCoord`).
    pub spout: Vec3,
    /// Whether it is spraying (`SpewWater`).
    pub spraying: bool,
    /// Seconds of spray left (`WaterTimer`).
    spray_timer: f32,
    /// Seconds until it can spray again (`RefillTimer`); it can at zero
    /// or below.
    refill_timer: f32,
    /// Seconds since the last burst (`SpewWaterRegulator`).
    burst_timer: f32,
    drops: Option<ParticleGroupId>,
}

impl KingWaterPipe {
    fn new(id: u8, spout: Vec3) -> Self {
        Self {
            id,
            spout,
            spraying: false,
            spray_timer: 0.0,
            refill_timer: 0.0,
            burst_timer: 0.0,
            drops: None,
        }
    }

    /// Starts the spray if the pipe is full and not spraying already. Port
    /// of `KickKingWaterPipe`.
    fn start_spray(&mut self) -> bool {
        if self.refill_timer > 0.0 || self.spraying {
            return false;
        }
        self.spraying = true;
        self.spray_timer = SPRAY_TIME;
        true
    }

    /// Advances the timers by `dt`; true when a burst of spray is due.
    /// Port of `MoveKingWaterPipe`, apart from the burst itself.
    fn tick(&mut self, dt: f32) -> bool {
        if !self.spraying {
            self.refill_timer -= dt;
            return false;
        }
        self.burst_timer += dt;
        let due = self.burst_timer > SPRAY_INTERVAL;
        if due {
            self.burst_timer = 0.0;
        }
        self.spray_timer -= dt;
        if self.spray_timer <= 0.0 {
            self.spraying = false;
            self.refill_timer = REFILL_TIME;
        }
        due
    }
}

/// Port of `AddKingWaterPipe`. `params[0]` is the pipe's number. The pipe
/// never goes out of range: its move function has no `TrackTerrainItem`.
fn add_king_water_pipe(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if level.0 != ANT_KING_LEVEL {
        warn!("King water pipes don't belong on level {}", level.0);
        return false;
    }
    let (x, z) = (spawn.position.x, spawn.position.y);
    let position = Vec3::new(x, map.floor_height(x, z), z);
    let pipe = commands
        .spawn((
            Name::new("King water pipe"),
            Transform::from_translation(position),
            Visibility::default(),
            KingWaterPipe::new(spawn.params[0], position + Vec3::Y * SPRAY_HEIGHT),
            TerrainItemSource(spawn.index),
            DespawnOnExit(AppState::InGame),
            Trigger {
                sides: SolidSides::ALL,
                solid: true,
            },
            solid_object(
                vec![KING_PIPE_BOX],
                LayerMask::from([
                    CollisionKind::Misc,
                    CollisionKind::Trigger,
                    CollisionKind::PlayerTriggerOnly,
                    CollisionKind::AutoTarget,
                    CollisionKind::Kickable,
                    CollisionKind::Impenetrable,
                ]),
                SolidSides::ALL,
            ),
        ))
        .id();
    models.spawn(
        &mut commands,
        pipe,
        KING_PIPE_MODEL,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(KING_PIPE_SCALE)),
    );
    true
}

/// Starts the spray of the pipes the ball rams or the bug kicks.
/// Port of `DoTrig_KingWaterPipe` and `KickKingWaterPipe`.
fn hit_king_water_pipes(
    mut hits: MessageReader<TriggerHit>,
    mut kicks: MessageReader<ItemKicked>,
    mut pipes: Query<&mut KingWaterPipe>,
    players: Query<(&PlayerForm, &PlayerSpeed), With<Player>>,
) {
    // A pipe clangs once per tick, however many hit it.
    let mut clanged = EntityHashSet::default();
    for hit in hits.read() {
        let Ok(mut pipe) = pipes.get_mut(hit.trigger) else {
            continue;
        };
        let Ok((form, speed)) = players.get(hit.mover) else {
            continue;
        };
        if **speed < RAM_SPEED || *form != PlayerForm::Ball {
            continue;
        }
        if pipe.start_spray() {
            // The ram starts a new group; the kick keeps the old one.
            pipe.drops = None;
        }
        if clanged.insert(hit.trigger) {
            // Sound: EFFECT_PIPECLANG at the spout.
        }
    }
    for kick in kicks.read() {
        let Ok(mut pipe) = pipes.get_mut(kick.item) else {
            continue;
        };
        pipe.start_spray();
        if clanged.insert(kick.item) {
            // Sound: EFFECT_PIPECLANG at the spout.
        }
    }
}

/// Sprays the water of the pipes that were set off, and refills them
/// afterwards. Port of `MoveKingWaterPipe`.
fn spray_king_water_pipes(
    time: Res<Time>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut pipes: Query<&mut KingWaterPipe>,
) {
    for mut pipe in &mut pipes {
        let was_spraying = pipe.spraying;
        let due = pipe.tick(time.delta_secs());
        if !was_spraying {
            // Sound: stop EFFECT_WATERLEAK.
            continue;
        }
        // Sound: EFFECT_WATERLEAK at the spout, three notes down, at 1.5
        // volume, started or kept up.
        if !due {
            continue;
        }
        let at = pipe.spout;
        emit_burst(
            &mut groups,
            &mut random,
            &mut pipe.drops,
            SPRAY_GROUP,
            SPRAY_DROPS_PER_BURST,
            true,
            |random, _| {
                let x = (random.next_f32() - 0.5) * SPRAY_SIDEWAYS_SPEED;
                let z = (random.next_f32() - 0.5) * SPRAY_SIDEWAYS_SPEED;
                (at, Vec3::new(x, SPRAY_RISE, z), SPRAY_DROP_SCALE)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_pipe_sprays_for_four_seconds_then_refills_for_five() {
        let mut pipe = KingWaterPipe::new(0, Vec3::ZERO);
        assert!(pipe.start_spray());
        assert!(!pipe.start_spray());
        let mut seconds = 0.0;
        while pipe.spraying {
            pipe.tick(DT);
            seconds += DT;
        }
        assert!((seconds - SPRAY_TIME).abs() < 2.0 * DT);
        // Still refilling.
        assert!(!pipe.start_spray());
        for _ in 0..(REFILL_TIME / DT) as usize + 2 {
            pipe.tick(DT);
        }
        assert!(pipe.start_spray());
    }

    #[test]
    fn bursts_come_every_few_ticks() {
        let mut pipe = KingWaterPipe::new(0, Vec3::ZERO);
        pipe.start_spray();
        let bursts = (0..60).filter(|_| pipe.tick(DT)).count();
        // Just over 0.03 s apart: every second tick at 60 fps.
        assert_eq!(bursts, 30);
    }

    fn world_with_pipe() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<TriggerHit>>();
        world.init_resource::<Messages<ItemKicked>>();
        let pipe = world.spawn(KingWaterPipe::new(0, Vec3::ZERO)).id();
        let player = world
            .spawn((Player, PlayerForm::Bug, PlayerSpeed(1000.0)))
            .id();
        (world, pipe, player)
    }

    fn ram(world: &mut World, pipe: Entity, player: Entity) -> bool {
        world.write_message(TriggerHit {
            trigger: pipe,
            mover: player,
            sides: SolidSides::ALL,
        });
        world.run_system_once(hit_king_water_pipes).unwrap();
        world.get::<KingWaterPipe>(pipe).unwrap().spraying
    }

    #[test]
    fn only_a_fast_ball_sets_it_off() {
        let (mut world, pipe, player) = world_with_pipe();
        assert!(!ram(&mut world, pipe, player));
        world.entity_mut(player).insert(PlayerForm::Ball);
        world.entity_mut(player).insert(PlayerSpeed(400.0));
        assert!(!ram(&mut world, pipe, player));
        world.entity_mut(player).insert(PlayerSpeed(600.0));
        assert!(ram(&mut world, pipe, player));
    }

    #[test]
    fn a_kick_sets_it_off() {
        let (mut world, pipe, player) = world_with_pipe();
        world.write_message(ItemKicked {
            player,
            item: pipe,
            direction: Vec2::X,
        });
        world.run_system_once(hit_king_water_pipes).unwrap();
        assert!(world.get::<KingWaterPipe>(pipe).unwrap().spraying);
    }

    #[test]
    fn the_spray_puts_out_fire() {
        assert!(SPRAY_GROUP.flags.contains(ParticleFlags::EXTINGUISH));
    }
}
