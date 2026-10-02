//! The nut, what is inside it, and the powerups it holds.
//!
//! Port of `AddNut`, `MoveNut`, `DoTrig_Nut`, `KickNut`,
//! `CreateNutContents`, `MakePowerup`, `MovePowerup`, `DoTrig_Powerup`,
//! `ShowThePOW` and `MovePowerupShow` (original/src/Items/Triggers.c).

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{DetonatorsBlown, MaterialOverride, SpawnBuddy, SpawnTick};
use crate::collision::{
    CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::combat::Health;
use crate::effects::{
    EffectsSystems, FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture,
};
use crate::items::kind as item;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemWindow, RegisterItemKind, TerrainItemSource,
    forget_terrain_item,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectModel, Shading, attach_shadow};
use crate::physics::Velocity;
use crate::player::{
    BallTime, DoorKey, Inventory, ItemKicked, PLAYER_MAX_HEALTH, Player, PlayerForm, PlayerSpeed,
    PlayerSystems, SHIELD_TIME, ShieldTimer,
};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.add_message::<SpawnTick>()
        .add_message::<SpawnBuddy>()
        .register_item_kind(item::NUT, add_nut)
        .add_systems(
            FixedUpdate,
            (blow_up_nuts, crack_nuts, collect_powerups, move_powerups)
                .chain()
                .after(PlayerSystems::Move)
                .before(EffectsSystems::MoveParticles)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `GLOBAL1_MObjType_Nut`
const NUT_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 2);
const NUT_SCALE: f32 = 1.7;
const NUT_BOX: CollisionBox = CollisionBox::new(110.0, 0.0, -60.0, 60.0, 60.0, -60.0);
const NUT_SHADOW_SCALE: f32 = 5.0;
/// What a nut leaves of the horizontal velocity of a player that bumps
/// into its side.
const NUT_SIDE_SLOWDOWN: f32 = 0.2;
/// How fast the ball must go to smash a nut, in units per second.
const NUT_SMASH_SPEED: f32 = 800.0;
/// `LEVEL_NUM_BEACH`, whose map makes every nut regenerate by mistake.
const BEACH_LEVEL: usize = 3;

const POWERUP_SCALE: f32 = 0.5;
const POWERUP_BOX: CollisionBox = CollisionBox::new(50.0, 0.0, -25.0, 25.0, 25.0, -25.0);
/// How fast powerups turn, in radians per second.
const POWERUP_SPIN_RATE: f32 = 4.0;
/// How far beyond the item window a lying powerup is kept, in units
/// (`TrackTerrainItem_Far`).
const POWERUP_TRACK_RANGE: f32 = 500.0;
/// Health a berry gives.
const BERRY_HEALTH: f32 = 0.5;
/// Ball time a mushroom gives, and the most there can be.
const MUSHROOM_BALL_TIME: f32 = 1.0;
const MAX_BALL_TIME: f32 = 1.0;
/// Seconds a collected powerup is shown rising before it goes.
const SHOW_TIME: f32 = 2.0;
/// Upward acceleration of a collected powerup, in units per second².
const SHOW_RISE_ACCELERATION: f32 = 600.0;

/// Seconds between the mushroom's bursts of particles.
const MUSHROOM_PARTICLE_INTERVAL: f32 = 0.05;
const MUSHROOM_PARTICLES_PER_BURST: usize = 3;
/// Where the particles start around the mushroom, in units: the size of the
/// random spread and the height above it.
const MUSHROOM_PARTICLE_SPREAD: Vec3 = Vec3::new(100.0, 30.0, 100.0);
const MUSHROOM_PARTICLE_HEIGHT: f32 = 100.0;
/// The particles' random velocity spread and upward drift, in units per
/// second.
const MUSHROOM_PARTICLE_SPEED: Vec3 = Vec3::new(40.0, 30.0, 40.0);
const MUSHROOM_PARTICLE_RISE: f32 = 30.0;
const MUSHROOM_PARTICLES: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Gravitoids,
    flags: ParticleFlags::NONE,
    gravity: 0.0,
    magnetism: 8000.0,
    base_scale: 5.0,
    decay_rate: 0.9,
    fade_rate: 0.0,
    texture: ParticleTexture::GreenRing,
};

/// Object types in `MODEL_GROUP_GLOBAL2` (`GLOBAL2_MObjType_*`).
mod global2 {
    pub const BERRY: usize = 0;
    pub const BALL_TIME: usize = 2;
    pub const FREE_LIFE: usize = 3;
    pub const SHIELD: usize = 4;
    pub const GOLD_CLOVER: usize = 5;
    pub const GREEN_CLOVER: usize = 6;
    pub const BLUE_CLOVER: usize = 7;
}
/// `LAWN1_MObjType_Key_Green` and `NIGHT_MObjType_Key_Green`; the other
/// colours follow.
const LAWN_KEY_MODEL: usize = 6;
const NIGHT_KEY_MODEL: usize = 18;
/// `POND_MObjType_Money`, which the original loads from whatever level
/// model file is current.
const MONEY_MODEL: usize = 9;

/// What a nut holds (`NUT_CONTENTS_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NutContents {
    BallTime,
    Key,
    Money,
    Health,
    FreeLife,
    Buddy,
    GreenClover,
    GoldClover,
    BlueClover,
    Shield,
    Tick,
}

impl NutContents {
    const ALL: [Self; 11] = [
        Self::BallTime,
        Self::Key,
        Self::Money,
        Self::Health,
        Self::FreeLife,
        Self::Buddy,
        Self::GreenClover,
        Self::GoldClover,
        Self::BlueClover,
        Self::Shield,
        Self::Tick,
    ];

    pub fn from_number(number: u8) -> Option<Self> {
        Self::ALL.get(usize::from(number)).copied()
    }

    /// Whether a lying powerup of this kind goes when it gets far away.
    /// Keys, money and the rarer clovers stay until collected.
    fn is_tracked(self) -> bool {
        matches!(
            self,
            Self::BallTime | Self::Health | Self::FreeLife | Self::GreenClover | Self::Shield
        )
    }
}

/// A nut on the map.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct Nut {
    /// `None` for a number the original doesn't know, which it only
    /// notices when the nut is cracked.
    contents: Option<NutContents>,
    /// The key's number, for a key (`NutParm1`).
    param: u8,
    /// Whether it comes back after being cracked (`RegenerateNut`).
    regenerate: bool,
    /// The detonator that blows it up on the hive levels, if any.
    detonator: Option<u8>,
}

impl Nut {
    /// Reads a nut's map item: `params[0]` is what it holds, `params[1]`
    /// the key number, `params[2]` its detonator, and `params[3]` bit 0
    /// makes it regenerate and bit 1 lets the detonator blow it up.
    fn from_item(params: [u8; 4], level: CurrentLevel) -> Self {
        Self {
            contents: NutContents::from_number(params[0]),
            param: params[1],
            // A fix for the Beach map, whose nuts all have the bit set.
            regenerate: *level != BEACH_LEVEL && params[3] & 1 != 0,
            detonator: (params[3] & (1 << 1) != 0).then_some(params[2]),
        }
    }
}

/// A powerup out of a nut.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct Powerup {
    contents: NutContents,
    /// The key's number, for a key (`KeyNum`).
    key: u8,
    state: PowerupState,
    /// Its turn about y (`Rot.y`).
    spin: f32,
    /// The model child, which turns.
    model: Option<Entity>,
    /// Seconds since the last burst of particles (`ParticleTimer`).
    particle_timer: f32,
    particles: Option<ParticleGroupId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PowerupState {
    /// Lying where the nut was (`MovePowerup`).
    Waiting,
    /// Collected and rising away (`MovePowerupShow`).
    Showing {
        /// Seconds left; it fades over the last one (`Health`).
        time_left: f32,
        /// Upward speed, in units per second.
        rise_speed: f32,
    },
}

/// Port of `AddNut`. Nuts holding the buddy bug are always added: the
/// buddy isn't ported, so there is never one already about.
fn add_nut(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
) -> bool {
    let nut = Nut::from_item(spawn.params, *level);
    if level.def().level_type == LevelType::Hive
        && nut.detonator.is_some_and(|id| blown.is_blown(id))
    {
        // Blown up already: it counts as added, so it isn't tried again.
        return true;
    }
    let (x, z) = (spawn.position.x, spawn.position.y);
    let root = commands
        .spawn((
            Name::new("Nut"),
            Transform::from_xyz(x, map.floor_height(x, z), z),
            Visibility::default(),
            nut,
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
            Trigger {
                sides: SolidSides::ALL,
                solid: true,
            },
            solid_object(
                vec![NUT_BOX],
                [
                    CollisionKind::Trigger,
                    CollisionKind::PlayerTriggerOnly,
                    CollisionKind::Kickable,
                    CollisionKind::BlockCamera,
                ],
                SolidSides::ALL,
            ),
        ))
        .id();
    models.spawn(
        &mut commands,
        root,
        NUT_MODEL,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(NUT_SCALE)),
    );
    attach_shadow(
        &mut commands,
        &mut models,
        root,
        Vec2::splat(NUT_SHADOW_SCALE),
        false,
    );
    true
}

/// Blows up the nuts of a detonator that has gone off. Port of the hive
/// part of `MoveNut`; the rest of it is [`DespawnOutOfRange`] and the
/// shadow.
fn blow_up_nuts(
    mut commands: Commands,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
    nuts: Query<(Entity, &Nut)>,
) {
    if level.def().level_type != LevelType::Hive {
        return;
    }
    for (entity, nut) in &nuts {
        if nut.detonator.is_some_and(|id| blown.is_blown(id)) {
            let mut nut = commands.entity(entity);
            forget_terrain_item(&mut nut);
            nut.despawn();
        }
    }
}

/// What cracking a nut needs.
#[derive(SystemParam)]
struct NutCracker<'w, 's> {
    commands: Commands<'w, 's>,
    models: ModelSpawner<'w>,
    level: Res<'w, CurrentLevel>,
    ticks: MessageWriter<'w, SpawnTick>,
    buddies: MessageWriter<'w, SpawnBuddy>,
}

impl NutCracker<'_, '_> {
    /// Makes what is inside and removes the shell, for good unless the nut
    /// regenerates. Port of the end of `DoTrig_Nut` and `KickNut`.
    fn crack(&mut self, entity: Entity, nut: &Nut, at: Vec3, player: Entity) {
        self.create_contents(nut, at, player);
        let mut shell = self.commands.entity(entity);
        if !nut.regenerate {
            forget_terrain_item(&mut shell);
        }
        shell.despawn();
    }

    /// Port of `CreateNutContents`. The buddy bug isn't ported, so there is
    /// never one already, and its nut always holds it rather than a green
    /// clover.
    fn create_contents(&mut self, nut: &Nut, at: Vec3, player: Entity) {
        match nut.contents {
            Some(NutContents::Buddy) => {
                self.buddies.write(SpawnBuddy {
                    player,
                    position: at,
                });
            }
            Some(NutContents::Tick) => {
                self.ticks.write(SpawnTick { position: at });
            }
            Some(contents) => self.make_powerup(contents, nut.param, at),
            None => warn!("A nut holds something unknown"),
        }
    }

    /// Port of `MakePowerup`.
    fn make_powerup(&mut self, contents: NutContents, param: u8, at: Vec3) {
        let level_type = self.level.def().level_type;
        let Some(model) = powerup_model(contents, level_type, param) else {
            return;
        };
        let root = self
            .commands
            .spawn((
                Name::new("Powerup"),
                Transform::from_translation(at),
                Visibility::default(),
                TransformInterpolation,
                DespawnOnExit(AppState::InGame),
                Trigger {
                    sides: SolidSides::ALL,
                    solid: false,
                },
                solid_object(
                    vec![POWERUP_BOX],
                    [
                        CollisionKind::Trigger,
                        CollisionKind::PlayerTriggerOnly,
                        CollisionKind::BlockCamera,
                    ],
                    SolidSides::ALL,
                ),
            ))
            .id();
        let model = self.models.spawn(
            &mut self.commands,
            root,
            model,
            Shading::Lit,
            Transform::from_scale(Vec3::splat(POWERUP_SCALE)),
        );
        self.commands.entity(root).insert(Powerup {
            contents,
            key: param,
            state: PowerupState::Waiting,
            spin: 0.0,
            model,
            particle_timer: 0.0,
            particles: None,
        });
    }
}

/// The model of a powerup (`MakePowerup`'s switch). Keys exist on the Lawn
/// and Night levels; elsewhere the original picks from the start of the
/// level's model file, as this does.
fn powerup_model(contents: NutContents, level_type: LevelType, key: u8) -> Option<ModelRef> {
    let global2 = |object| Some(ModelRef::new(ModelFile::Global2, object));
    match contents {
        NutContents::BallTime => global2(global2::BALL_TIME),
        NutContents::Key => {
            let first = match level_type {
                LevelType::Lawn => LAWN_KEY_MODEL,
                LevelType::Night => NIGHT_KEY_MODEL,
                _ => 0,
            };
            Some(ModelRef::new(ModelFile::Level1, first + usize::from(key)))
        }
        NutContents::Money => Some(ModelRef::new(ModelFile::Level1, MONEY_MODEL)),
        NutContents::Health => global2(global2::BERRY),
        NutContents::FreeLife => global2(global2::FREE_LIFE),
        NutContents::GreenClover => global2(global2::GREEN_CLOVER),
        NutContents::GoldClover => global2(global2::GOLD_CLOVER),
        NutContents::BlueClover => global2(global2::BLUE_CLOVER),
        NutContents::Shield => global2(global2::SHIELD),
        NutContents::Buddy | NutContents::Tick => {
            warn!("{contents:?} isn't a powerup");
            None
        }
    }
}

/// Cracks the nuts the bug kicks or the ball smashes, and slows down a
/// player that bumps into a nut's side.
///
/// Port of `DoTrig_Nut` and `KickNut`. The slowdown applies to the
/// velocity the player's move left, so from the next tick on, where the
/// original slows the player in the middle of its move.
fn crack_nuts(
    mut hits: MessageReader<TriggerHit>,
    mut kicks: MessageReader<ItemKicked>,
    nuts: Query<(&Nut, &Transform)>,
    mut players: Query<(&PlayerForm, &PlayerSpeed, &mut Velocity), With<Player>>,
    mut cracker: NutCracker,
) {
    let mut cracked = EntityHashSet::default();
    for hit in hits.read() {
        let Ok((nut, transform)) = nuts.get(hit.trigger) else {
            continue;
        };
        let Ok((form, speed, mut velocity)) = players.get_mut(hit.mover) else {
            continue;
        };
        if !hit.sides.intersects(SolidSides::BOTTOM) {
            velocity.x *= NUT_SIDE_SLOWDOWN;
            velocity.z *= NUT_SIDE_SLOWDOWN;
        }
        if *form == PlayerForm::Ball && **speed > NUT_SMASH_SPEED && cracked.insert(hit.trigger) {
            // Sound: EFFECT_POP at the nut.
            cracker.crack(hit.trigger, nut, transform.translation, hit.mover);
        }
    }
    for kick in kicks.read() {
        let Ok((nut, transform)) = nuts.get(kick.item) else {
            continue;
        };
        if cracked.insert(kick.item) {
            // Sound: EFFECT_POP at the kicker.
            cracker.crack(kick.item, nut, transform.translation, kick.player);
        }
    }
}

/// Gives a player what a powerup holds. Port of `DoTrig_Powerup`'s switch.
fn give_powerup(
    contents: NutContents,
    key: u8,
    inventory: &mut Inventory,
    health: &mut Health,
    ball_time: &mut BallTime,
    shield: &mut ShieldTimer,
) {
    match contents {
        NutContents::Key => match DoorKey::from_number(key) {
            Some(key) => inventory.get_key(key),
            None => warn!("A powerup holds key {key}, which doesn't exist"),
        },
        NutContents::Money => inventory.get_money(),
        NutContents::Health => health.gain(BERRY_HEALTH, PLAYER_MAX_HEALTH),
        NutContents::BallTime => {
            **ball_time = (**ball_time + MUSHROOM_BALL_TIME).min(MAX_BALL_TIME)
        }
        NutContents::FreeLife => inventory.get_life(),
        NutContents::GreenClover => inventory.get_green_clover(),
        NutContents::GoldClover => inventory.get_gold_clover(),
        NutContents::BlueClover => inventory.get_blue_clover(),
        NutContents::Shield => shield.0 = SHIELD_TIME,
        NutContents::Buddy | NutContents::Tick => {}
    }
}

/// Collects the powerups players touch and shows them rising away.
/// Port of `DoTrig_Powerup` and `ShowThePOW`.
fn collect_powerups(
    mut commands: Commands,
    mut hits: MessageReader<TriggerHit>,
    mut powerups: Query<&mut Powerup>,
    mut players: Query<
        (&mut Inventory, &mut Health, &mut BallTime, &mut ShieldTimer),
        With<Player>,
    >,
) {
    for hit in hits.read() {
        let Ok(mut powerup) = powerups.get_mut(hit.trigger) else {
            continue;
        };
        // Two players can touch it in the same tick.
        if powerup.state != PowerupState::Waiting {
            continue;
        }
        let Ok((mut inventory, mut health, mut ball_time, mut shield)) = players.get_mut(hit.mover)
        else {
            continue;
        };
        give_powerup(
            powerup.contents,
            powerup.key,
            &mut inventory,
            &mut health,
            &mut ball_time,
            &mut shield,
        );
        // Sound: EFFECT_GETPOW at the powerup.
        powerup.state = PowerupState::Showing {
            time_left: SHOW_TIME,
            rise_speed: 0.0,
        };
        commands.entity(hit.trigger).remove::<Trigger>().insert((
            CollisionLayers::new(LayerMask::NONE, LayerMask::NONE),
            // `STATUS_BIT_GLOW | STATUS_BIT_NOZWRITE`
            MaterialOverride {
                glow: true,
                ..default()
            },
        ));
    }
}

/// Whether a position is beyond the item window by more than `range`.
///
/// Port of `IsPositionOutOfRange_Far` (original/src/Terrain/Terrain2.c),
/// including its sign slip on z: there it narrows the window by `range`
/// instead of widening it.
fn is_out_of_range_far(window: &ItemWindow, position: Vec2, range: f32) -> bool {
    position.x < window.delete_min.x - range
        || position.x > window.delete_max.x + range
        || position.y < window.delete_min.y + range
        || position.y > window.delete_max.y - range
}

/// Turns the powerups, sends up the mushroom's particles, and lifts and
/// fades the collected ones.
///
/// Port of `MovePowerup` and `MovePowerupShow`.
fn move_powerups(
    time: Res<Time>,
    mut commands: Commands,
    window: Option<Res<ItemWindow>>,
    mut random: ResMut<GameRandom>,
    mut groups: ResMut<ParticleGroups>,
    mut powerups: Query<(
        Entity,
        &mut Powerup,
        &mut Transform,
        Option<&mut MaterialOverride>,
    )>,
    mut models: Query<&mut Transform, (With<ObjectModel>, Without<Powerup>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut powerup, mut transform, material) in &mut powerups {
        match powerup.state {
            PowerupState::Waiting => {
                if powerup.contents.is_tracked()
                    && window.as_ref().is_some_and(|window| {
                        is_out_of_range_far(window, transform.translation.xz(), POWERUP_TRACK_RANGE)
                    })
                {
                    commands.entity(entity).despawn();
                    continue;
                }
                if powerup.contents == NutContents::BallTime {
                    emit_mushroom_particles(
                        &mut powerup,
                        transform.translation,
                        dt,
                        &mut groups,
                        &mut random,
                    );
                }
            }
            PowerupState::Showing {
                time_left,
                rise_speed,
            } => {
                let rise_speed = rise_speed + SHOW_RISE_ACCELERATION * dt;
                transform.translation.y += rise_speed * dt;
                let time_left = time_left - dt;
                if time_left <= 0.0 {
                    commands.entity(entity).despawn();
                    continue;
                }
                if time_left < 1.0
                    && let Some(mut material) = material
                {
                    material.opacity = time_left;
                }
                powerup.state = PowerupState::Showing {
                    time_left,
                    rise_speed,
                };
            }
        }
        powerup.spin += POWERUP_SPIN_RATE * dt;
        if let Some(mut model) = powerup.model.and_then(|m| models.get_mut(m).ok()) {
            model.rotation = Quat::from_rotation_y(powerup.spin);
        }
    }
}

/// The mushroom's green rings. Port of the particle part of `MovePowerup`.
fn emit_mushroom_particles(
    powerup: &mut Powerup,
    at: Vec3,
    dt: f32,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
) {
    powerup.particle_timer += dt;
    if powerup.particle_timer <= MUSHROOM_PARTICLE_INTERVAL {
        return;
    }
    powerup.particle_timer = 0.0;
    if powerup.particles.is_none_or(|id| !groups.is_valid(id)) {
        powerup.particles = groups.new_group(MUSHROOM_PARTICLES);
    }
    let Some(id) = powerup.particles else {
        return;
    };
    for _ in 0..MUSHROOM_PARTICLES_PER_BURST {
        let mut centred = || random.next_f32() - 0.5;
        let offset = Vec3::new(centred(), centred(), centred()) * MUSHROOM_PARTICLE_SPREAD;
        let position = at + offset + Vec3::Y * MUSHROOM_PARTICLE_HEIGHT;
        let velocity = Vec3::new(centred(), centred(), centred()) * MUSHROOM_PARTICLE_SPEED
            + Vec3::Y * MUSHROOM_PARTICLE_RISE;
        let scale = random.next_f32() + 1.0;
        groups.add_particle(id, position, velocity, scale, FULL_ALPHA);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn nut_items_are_read_like_the_original() {
        let lawn = CurrentLevel(1);
        let nut = Nut::from_item([1, 3, 7, 0b11], lawn);
        assert_eq!(
            nut,
            Nut {
                contents: Some(NutContents::Key),
                param: 3,
                regenerate: true,
                detonator: Some(7),
            }
        );
        // The Beach's nuts never regenerate.
        let beach = Nut::from_item([6, 0, 0, 1], CurrentLevel(BEACH_LEVEL));
        assert!(!beach.regenerate);
        assert_eq!(beach.contents, Some(NutContents::GreenClover));
        assert_eq!(beach.detonator, None);
        assert_eq!(Nut::from_item([11, 0, 0, 0], lawn).contents, None);
        assert_eq!(NutContents::from_number(10), Some(NutContents::Tick));
    }

    #[test]
    fn powerups_fill_the_inventory() {
        let mut inventory = Inventory::default();
        let mut health = Health(0.8);
        let mut ball_time = BallTime(0.5);
        let mut shield = ShieldTimer(0.0);
        let mut give = |contents, key| {
            give_powerup(
                contents,
                key,
                &mut inventory,
                &mut health,
                &mut ball_time,
                &mut shield,
            );
        };
        give(NutContents::Key, 2);
        give(NutContents::Health, 0);
        give(NutContents::BallTime, 0);
        give(NutContents::Shield, 0);
        give(NutContents::Money, 0);
        give(NutContents::FreeLife, 0);
        give(NutContents::GoldClover, 0);
        assert!(inventory.has_key(DoorKey::Red));
        assert_eq!(inventory.money, 1);
        assert_eq!(inventory.lives, crate::player::STARTING_LIVES + 1);
        assert_eq!(inventory.gold_clovers, 1);
        assert_eq!(health, Health(PLAYER_MAX_HEALTH));
        assert_eq!(ball_time, BallTime(MAX_BALL_TIME));
        assert_eq!(shield, ShieldTimer(SHIELD_TIME));
    }

    #[test]
    fn powerup_models_follow_the_level() {
        assert_eq!(
            powerup_model(NutContents::Key, LevelType::Lawn, 2),
            Some(ModelRef::new(ModelFile::Level1, 8))
        );
        assert_eq!(
            powerup_model(NutContents::Key, LevelType::Night, 0),
            Some(ModelRef::new(ModelFile::Level1, 18))
        );
        assert_eq!(
            powerup_model(NutContents::GreenClover, LevelType::Pond, 0),
            Some(ModelRef::new(ModelFile::Global2, 6))
        );
        assert_eq!(powerup_model(NutContents::Tick, LevelType::Hive, 0), None);
    }

    #[test]
    fn a_collected_powerup_rises_fades_and_goes() {
        let mut world = World::new();
        world.insert_resource(GameRandom::from_seed(1));
        world.init_resource::<ParticleGroups>();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_millis(500));
        world.insert_resource(time);
        let powerup = world
            .spawn((
                Transform::default(),
                Powerup {
                    contents: NutContents::Key,
                    key: 0,
                    state: PowerupState::Showing {
                        time_left: SHOW_TIME,
                        rise_speed: 0.0,
                    },
                    spin: 0.0,
                    model: None,
                    particle_timer: 0.0,
                    particles: None,
                },
                MaterialOverride {
                    glow: true,
                    ..default()
                },
            ))
            .id();
        let step = |world: &mut World| {
            world
                .run_system_once(move_powerups)
                .expect("the system runs");
        };
        step(&mut world);
        // 600 units/s² for half a second, applied before the move.
        assert_eq!(
            world.get::<Transform>(powerup).map(|t| t.translation.y),
            Some(150.0)
        );
        step(&mut world);
        step(&mut world);
        let opacity = world.get::<MaterialOverride>(powerup).map(|m| m.opacity);
        assert_eq!(opacity, Some(0.5));
        step(&mut world);
        assert!(world.get_entity(powerup).is_err());
    }

    #[test]
    fn the_mushroom_sends_up_particles_in_bursts() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::from_seed(1);
        let mut powerup = Powerup {
            contents: NutContents::BallTime,
            key: 0,
            state: PowerupState::Waiting,
            spin: 0.0,
            model: None,
            particle_timer: 0.0,
            particles: None,
        };
        emit_mushroom_particles(&mut powerup, Vec3::ZERO, 0.04, &mut groups, &mut random);
        assert!(powerup.particles.is_none());
        emit_mushroom_particles(&mut powerup, Vec3::ZERO, 0.04, &mut groups, &mut random);
        let id = powerup.particles.expect("a group");
        let particles = groups.get(id).map(|g| g.particles().len());
        assert_eq!(particles, Some(MUSHROOM_PARTICLES_PER_BURST));
        assert_eq!(powerup.particle_timer, 0.0);
    }
}
