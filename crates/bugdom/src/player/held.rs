//! The bug held or trapped by an enemy: swallowed by a pond fish or a bat,
//! carried off by a firefly, sucked by a mosquito or caught in a spider's
//! web.
//!
//! Port of `MovePlayerBug_BeingEaten`, `MovePlayerBug_Carried`,
//! `MovePlayerBug_BloodSuck` and `MovePlayerBug_Webbed`
//! (original/src/Player/Player_Bug.c), and of the player's side of the
//! enemies that start and end them (Enemy_PondFish.c, Traps.c,
//! Enemy_FireFly.c, Enemy_Mosquito.c and Enemy_Spider.c in
//! original/src/Enemies and original/src/Items).
//!
//! The original's enemies change the player's animation and `CType`
//! directly, and keep who holds it in globals (`gCurrentEatingFish`,
//! `gCurrentEatingBat`, `gCurrentCarryingFireFly`). Here they send
//! [`HoldPlayer`] and [`ReleasePlayer`], and who holds the player is in
//! [`EatenBy`] and [`CarriedBy`] on the player. Each state's movement is in
//! `bug.rs` with the others; following the eater's mouth is
//! [`follow_eaters`].

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::ball::become_bug;
use super::bug::BugState;
use super::movement::{PlayerData, PlayerDataItem};
use super::{KillPlayer, PLAYER_BUG_SCALE, Player, PlayerForm, PlayerModel};
use crate::collision::CollisionKind;
use crate::enemies::EnemyModel;
use crate::skeleton::SkeletonRig;

/// How far below its carrier a carried bug hangs, in units
/// (`MovePlayerBug_Carried`).
pub const CARRIED_DROP: f32 = 180.0;

/// The player is in an enemy's mouth (`gCurrentEatingFish`,
/// `gCurrentEatingBat`): its model sits at `mouth_offset` in the space of
/// the enemy's `joint`. It stays there until the player starts again, even
/// once it counts as dead.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct EatenBy {
    pub enemy: Entity,
    /// The joint of the enemy's skeleton that holds the mouth
    /// (`PONDFISH_JOINT_HEAD`, `BAT_JOINT_HEAD`).
    pub joint: usize,
    /// The mouth in that joint's space, before the enemy's scale
    /// (`gPondFishMouthOff`, `gBatMouthOff`).
    pub mouth_offset: Vec3,
    /// The player's position follows the mouth, and the camera and item
    /// window with it. The bat sets it; the pond fish doesn't, so on the
    /// Pond the player stays where it was caught (the `LEVEL_TYPE_FOREST`
    /// case in `MovePlayerBug_BeingEaten`).
    pub follow: bool,
}

/// The player hangs under this enemy (`gCurrentCarryingFireFly`). The
/// carrier lets go by sending [`ReleasePlayer`], or by going away.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarriedBy(pub Entity);

/// What an enemy does to the player.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hold {
    /// Swallowed: [`BugState::BeingEaten`] in the enemy's mouth
    /// (`SeeIfFishEatsPlayer`, `SeeIfBatEatsPlayer`).
    Eaten {
        by: Entity,
        joint: usize,
        mouth_offset: Vec3,
        /// See [`EatenBy::follow`].
        follow: bool,
    },
    /// Carried off: [`BugState::Carried`] (`FireFlyChasePlayer`).
    Carried { by: Entity },
    /// Stung: [`BugState::BloodSuck`], which stops the player
    /// (`MoveMosquito_Dive`).
    BloodSuck,
    /// Caught in a web: [`BugState::Webbed`] (`WebBulletHitCallback`).
    Webbed,
}

impl Hold {
    /// The state the hold puts the bug in.
    pub const fn state(self) -> BugState {
        match self {
            Self::Eaten { .. } => BugState::BeingEaten,
            Self::Carried { .. } => BugState::Carried,
            Self::BloodSuck => BugState::BloodSuck,
            Self::Webbed => BugState::Webbed,
        }
    }

    /// Whether others stop colliding with the player (`CType = 0`). The
    /// original sets it when the bug is eaten, and on every tick of the
    /// blood suck and the web; a carried bug keeps its collision.
    pub const fn hides_from_collision(self) -> bool {
        !matches!(self, Self::Carried { .. })
    }
}

/// Puts a player in an enemy's hold. A ball turns into the bug first, as
/// the original's enemies call `InitPlayer_Bug` with the hold's animation.
/// Applied once every object has moved, in
/// [`PlayerSystems::Hold`](super::PlayerSystems::Hold).
///
/// A dead player can't be held: the original's dead bug has no `CType`, so
/// no enemy finds it. Webbing a webbed bug does nothing
/// (`WebBulletHitCallback`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct HoldPlayer {
    pub player: Entity,
    pub hold: Hold,
}

/// Lets a held player go. A blood-sucked or webbed bug stands up again; a
/// carried bug falls on its next move (`MovePlayerBug_Carried` with no
/// `gCurrentCarryingFireFly`). The original never lets an eaten bug go: it
/// dies in the mouth and starts again, so this does nothing to it.
///
/// It only acts on a player still in a hold: unlike the original, a
/// mosquito killed while its victim has been knocked over doesn't stand
/// the victim up (`KillMosquito` morphs any bug to standing).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleasePlayer {
    pub player: Entity,
    /// Others collide with the player again (`CType = CTYPE_PLAYER`). Only
    /// the mosquito that finishes sucking does this in the original;
    /// `KillMosquito` and the web leave the player out of others'
    /// collisions until it turns into the ball or starts again.
    pub restore_collision: bool,
}

/// The player's collision kinds (`CType`): `CTYPE_PLAYER`, or none while
/// others must not find it.
pub fn player_layers(collidable: bool) -> CollisionLayers {
    let kinds = if collidable {
        LayerMask::from(CollisionKind::Player)
    } else {
        LayerMask::NONE
    };
    CollisionLayers::new(kinds, LayerMask::NONE)
}

/// Applies [`ReleasePlayer`], then [`HoldPlayer`].
///
/// Port of the player's half of the hold and release code in
/// `SeeIfFishEatsPlayer` (original/src/Enemies/Enemy_PondFish.c),
/// `SeeIfBatEatsPlayer` (original/src/Items/Traps.c),
/// `FireFlyChasePlayer` and `FireFlyCarryPlayer`
/// (original/src/Enemies/Enemy_FireFly.c), `MoveMosquito_Dive`,
/// `MoveMosquito_Suck` and `KillMosquito`
/// (original/src/Enemies/Enemy_Mosquito.c), and `WebBulletHitCallback`
/// and `MoveWebSphere` (original/src/Enemies/Enemy_Spider.c).
pub(super) fn apply_holds(
    mut releases: MessageReader<ReleasePlayer>,
    mut holds: MessageReader<HoldPlayer>,
    mut commands: Commands,
    mut players: Query<PlayerData, With<Player>>,
) {
    for release in releases.read() {
        let Ok(mut player) = players.get_mut(release.player) else {
            continue;
        };
        release_player(&mut player, &mut commands, release.restore_collision);
    }
    for hold in holds.read() {
        let Ok(mut player) = players.get_mut(hold.player) else {
            continue;
        };
        hold_player(&mut player, &mut commands, hold.hold);
    }
}

fn release_player(player: &mut PlayerDataItem, commands: &mut Commands, restore_collision: bool) {
    let mut entity = commands.entity(player.entity);
    if *player.form == PlayerForm::Bug {
        match *player.state {
            // `MoveMosquito_Suck`, `KillMosquito` and `MoveWebSphere`.
            BugState::BloodSuck | BugState::Webbed => *player.state = BugState::Stand,
            // The bug notices on its next move.
            BugState::Carried => {
                entity.remove::<CarriedBy>();
            }
            _ => {}
        }
    }
    if restore_collision {
        entity.insert(player_layers(true));
    }
}

fn hold_player(player: &mut PlayerDataItem, commands: &mut Commands, hold: Hold) {
    let state = hold.state();
    if player.dying {
        return;
    }
    if *player.form == PlayerForm::Ball {
        become_bug(player, state);
    } else if hold == Hold::Webbed && *player.state == BugState::Webbed {
        return;
    } else {
        *player.state = state;
    }
    let mut entity = commands.entity(player.entity);
    match hold {
        Hold::Eaten {
            by,
            joint,
            mouth_offset,
            follow,
        } => {
            entity.insert(EatenBy {
                enemy: by,
                joint,
                mouth_offset,
                follow,
            });
        }
        Hold::Carried { by } => {
            entity.insert(CarriedBy(by));
        }
        // The mosquito stops the bug where it stings it.
        Hold::BloodSuck => **player.velocity = Vec3::ZERO,
        Hold::Webbed => {}
    }
    if hold.hides_from_collision() {
        entity.insert(player_layers(false));
    }
}

/// Where an eaten bug's model is in the world, given the eater's joint
/// matrix (`FindJointFullMatrix`, which includes the eater's scale), the
/// eater's scale and the mouth in the joint's space.
///
/// As `MovePlayerBug_BeingEaten` composes it: the bug's scale with the
/// eater's taken out, then the joint, with the mouth offset scaled by the
/// eater's scale applied first. The offset ends up scaled by both, so the
/// bug sits a little beyond the mouth point itself; that is how the
/// original looks.
pub fn eaten_model_matrix(joint: Affine3A, eater_scale: f32, mouth_offset: Vec3) -> Affine3A {
    joint
        * Affine3A::from_scale(Vec3::splat(PLAYER_BUG_SCALE / eater_scale))
        * Affine3A::from_translation(mouth_offset * eater_scale)
}

/// Splits an eaten bug's model matrix between the player's root, which
/// moves to the model's origin and keeps its heading, and the model's
/// local transform under it.
pub(super) fn split_eaten_matrix(model: Affine3A, root_rotation: Quat) -> (Vec3, Transform) {
    let origin = Vec3::from(model.translation);
    (origin, model_relative_to(model, root_rotation, origin))
}

/// The model's transform relative to a root at `root_translation`.
pub(super) fn model_relative_to(
    model: Affine3A,
    root_rotation: Quat,
    root_translation: Vec3,
) -> Transform {
    let root = Affine3A::from_rotation_translation(root_rotation, root_translation);
    Transform::from_matrix(Mat4::from(root.inverse() * model))
}

/// The world matrix of an object's `joint` (`FindJointFullMatrix`) and
/// the object's scale. Where the skeleton and scale are on a model child
/// (an enemy's [`EnemyModel`], a root swing's model), `model` names it and
/// the root has a unit scale; otherwise they are on the object itself.
pub(super) fn joint_matrix(
    rigs: &Query<(&Transform, Option<&SkeletonRig>), Without<Player>>,
    object: Entity,
    model: Option<Entity>,
    joint: usize,
) -> Option<(Affine3A, f32)> {
    let (root, root_rig) = rigs.get(object).ok()?;
    let (base, scale, rig) = match model {
        Some(model) => {
            let (model_transform, rig) = rigs.get(model).ok()?;
            (
                root.compute_affine() * model_transform.compute_affine(),
                model_transform.scale.x,
                rig,
            )
        }
        None => (root.compute_affine(), root.scale.x, root_rig),
    };
    Some((rig?.joint_transform(joint, base)?, scale))
}

/// Keeps each eaten bug in its eater's mouth, and kills it if the eater is
/// gone. Where the eater asks for it ([`EatenBy::follow`]), the player's
/// root follows the mouth, so the camera and the item window follow it.
///
/// Port of `MovePlayerBug_BeingEaten` (original/src/Player/Player_Bug.c).
#[allow(clippy::type_complexity)]
pub(super) fn follow_eaters(
    mut kills: MessageWriter<KillPlayer>,
    mut players: Query<(Entity, &EatenBy, &BugState, &PlayerModel, &mut Transform), With<Player>>,
    eaters: Query<Option<&EnemyModel>>,
    mut transforms: ParamSet<(
        Query<(&Transform, Option<&SkeletonRig>), Without<Player>>,
        Query<&mut Transform, Without<Player>>,
    )>,
) {
    for (player, eaten, state, model, mut transform) in &mut players {
        if *state != BugState::BeingEaten {
            continue;
        }
        let Ok(eater_model) = eaters.get(eaten.enemy) else {
            kills.write(KillPlayer {
                player,
                change_anims: false,
            });
            continue;
        };
        let Some((joint, scale)) = joint_matrix(
            &transforms.p0(),
            eaten.enemy,
            eater_model.map(|m| m.0),
            eaten.joint,
        ) else {
            continue;
        };
        let matrix = eaten_model_matrix(joint, scale, eaten.mouth_offset);
        let local = if eaten.follow {
            let (origin, local) = split_eaten_matrix(matrix, transform.rotation);
            transform.translation = origin;
            local
        } else {
            model_relative_to(matrix, transform.rotation, transform.translation)
        };
        if let Ok(mut model_transform) = transforms.p1().get_mut(model.0) {
            *model_transform = local;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::collision::{SolidSides, solid_object};
    use crate::combat::Health;
    use crate::physics::Velocity;
    use crate::player::{Dying, PlayerTuning};

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<PlayerTuning>();
        world.init_resource::<Messages<HoldPlayer>>();
        world.init_resource::<Messages<ReleasePlayer>>();
        world.init_resource::<Messages<KillPlayer>>();
        world
    }

    fn spawn_player(world: &mut World, form: PlayerForm, state: BugState) -> Entity {
        world
            .spawn((
                Player,
                Transform::from_xyz(100.0, 50.0, 100.0),
                solid_object(
                    vec![form.collision_box()],
                    CollisionKind::Player,
                    SolidSides::TOUCHABLE,
                ),
                form,
                state,
                Velocity(Vec3::new(300.0, 0.0, 0.0)),
                Health(1.0),
            ))
            .id()
    }

    /// Runs [`apply_holds`] once, then drops the messages, which a new
    /// system would otherwise read again.
    fn apply(world: &mut World) {
        world.run_system_once(apply_holds).expect("the system runs");
        world.resource_mut::<Messages<HoldPlayer>>().clear();
        world.resource_mut::<Messages<ReleasePlayer>>().clear();
    }

    fn hold(world: &mut World, player: Entity, hold: Hold) {
        world.write_message(HoldPlayer { player, hold });
        apply(world);
    }

    fn release(world: &mut World, player: Entity, restore_collision: bool) {
        world.write_message(ReleasePlayer {
            player,
            restore_collision,
        });
        apply(world);
    }

    fn collidable(world: &World, player: Entity) -> bool {
        world
            .get::<CollisionLayers>(player)
            .is_some_and(|l| l.memberships.has_all(CollisionKind::Player))
    }

    #[test]
    fn an_eaten_bug_leaves_others_collisions_and_remembers_its_eater() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::Swim);
        let fish = world.spawn_empty().id();
        let eaten = Hold::Eaten {
            by: fish,
            joint: 4,
            mouth_offset: Vec3::new(0.0, -17.0, -40.0),
            follow: true,
        };
        hold(&mut world, player, eaten);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::BeingEaten));
        assert_eq!(
            world.get::<EatenBy>(player),
            Some(&EatenBy {
                enemy: fish,
                joint: 4,
                mouth_offset: Vec3::new(0.0, -17.0, -40.0),
                follow: true,
            })
        );
        assert!(!collidable(&world, player));
        // The original never lets an eaten bug go.
        release(&mut world, player, false);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::BeingEaten));
    }

    #[test]
    fn a_ball_turns_into_the_bug_to_be_held() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Ball, BugState::RollUp);
        hold(&mut world, player, Hold::Webbed);
        assert_eq!(world.get::<PlayerForm>(player), Some(&PlayerForm::Bug));
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Webbed));
        // `InitPlayer_Bug` puts the bug's feet where the ball's bottom was.
        let y = world.get::<Transform>(player).map(|t| t.translation.y);
        assert_eq!(y, Some(50.0 - crate::player::PLAYER_BALL_FOOT_OFFSET));
        assert!(!collidable(&world, player));
    }

    #[test]
    fn the_blood_suck_stops_the_bug_and_ends_standing() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::Walk);
        hold(&mut world, player, Hold::BloodSuck);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::BloodSuck));
        assert_eq!(world.get::<Velocity>(player), Some(&Velocity(Vec3::ZERO)));
        assert!(!collidable(&world, player));
        // The mosquito that finishes sucking gives the collision back.
        release(&mut world, player, true);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Stand));
        assert!(collidable(&world, player));
    }

    #[test]
    fn the_web_lets_go_without_giving_the_collision_back() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::Stand);
        hold(&mut world, player, Hold::Webbed);
        assert_eq!(world.get::<Velocity>(player).map(|v| v.x), Some(300.0));
        release(&mut world, player, false);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Stand));
        assert!(!collidable(&world, player));
        // Released twice, or not held: nothing happens.
        world.entity_mut(player).insert(BugState::KnockedOnButt);
        release(&mut world, player, false);
        assert_eq!(
            world.get::<BugState>(player),
            Some(&BugState::KnockedOnButt)
        );
    }

    #[test]
    fn a_carried_bug_keeps_its_collision_until_let_go() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::Stand);
        let firefly = world.spawn_empty().id();
        hold(&mut world, player, Hold::Carried { by: firefly });
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Carried));
        assert_eq!(world.get::<CarriedBy>(player), Some(&CarriedBy(firefly)));
        assert!(collidable(&world, player));
        // It falls on its next move.
        release(&mut world, player, false);
        assert!(world.get::<CarriedBy>(player).is_none());
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Carried));
    }

    #[test]
    fn the_dead_cant_be_held() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::Death);
        world.entity_mut(player).insert(Dying { timer: 1.0 });
        hold(&mut world, player, Hold::BloodSuck);
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Death));
        assert!(collidable(&world, player));
    }

    #[test]
    fn a_bug_whose_eater_is_gone_dies_where_it_is() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, BugState::BeingEaten);
        let model = world.spawn(Transform::default()).id();
        let gone = world.spawn_empty().id();
        world.despawn(gone);
        world.entity_mut(player).insert((
            PlayerModel(model),
            EatenBy {
                enemy: gone,
                joint: 0,
                mouth_offset: Vec3::ZERO,
                follow: false,
            },
        ));
        world
            .run_system_once(follow_eaters)
            .expect("the system runs");
        let kills: Vec<_> = world
            .resource_mut::<Messages<KillPlayer>>()
            .drain()
            .collect();
        assert_eq!(
            kills,
            [KillPlayer {
                player,
                change_anims: false
            }]
        );
    }

    #[test]
    fn without_following_the_root_stays_and_the_model_reaches_the_mouth() {
        let mouth = Affine3A::from_scale_rotation_translation(
            Vec3::splat(0.85),
            Quat::from_rotation_y(0.7),
            Vec3::new(500.0, 80.0, -300.0),
        );
        let (rotation, at) = (Quat::from_rotation_y(-1.2), Vec3::new(100.0, 0.0, 40.0));
        let local = model_relative_to(mouth, rotation, at);
        let world = Affine3A::from_rotation_translation(rotation, at) * local.compute_affine();
        assert!(world.abs_diff_eq(mouth, 1e-3), "{world:?}");
    }

    #[test]
    fn the_eaten_model_matrix_composes_like_the_original() {
        // A joint turned a quarter turn about Y, at (100, 200, 300), in an
        // eater of scale 2 (the joint matrix includes it).
        let eater_scale = 2.0;
        let joint = Affine3A::from_scale_rotation_translation(
            Vec3::splat(eater_scale),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(100.0, 200.0, 300.0),
        );
        let mouth = Vec3::new(0.0, -17.0, -40.0);
        let m = eaten_model_matrix(joint, eater_scale, mouth);

        // By hand: the bug's point p goes to
        // joint(s · (p + mouth · 2)) with s = 1.7 / 2, so the joint's
        // scale of 2 leaves the bug at 1.7. The origin goes to
        // joint(0.85 · (0, −34, −80)) = joint((0, −28.9, −68)); the joint
        // scales by 2 to (0, −57.8, −136) and turns it a quarter turn about
        // Y, taking −Z to −X: (−136, −57.8, 0), then moves it.
        let origin = m.transform_point3(Vec3::ZERO);
        assert!(
            origin.abs_diff_eq(Vec3::new(100.0 - 136.0, 200.0 - 57.8, 300.0), 1e-3),
            "{origin}"
        );
        // A unit along the bug's X is 1.7 long and turned onto −Z.
        let x = m.transform_vector3(Vec3::X);
        assert!(x.abs_diff_eq(Vec3::new(0.0, 0.0, -1.7), 1e-4), "{x}");
        let y = m.transform_vector3(Vec3::Y);
        assert!(y.abs_diff_eq(Vec3::new(0.0, 1.7, 0.0), 1e-4), "{y}");

        // The split keeps the whole: the root at the origin with its own
        // heading, and the model's local transform under it.
        let heading = Quat::from_rotation_y(0.3);
        let (root, local) = split_eaten_matrix(m, heading);
        assert!(root.abs_diff_eq(origin, 1e-3));
        let whole = Affine3A::from_rotation_translation(heading, root) * local.compute_affine();
        assert!(whole.abs_diff_eq(m, 1e-3));
        assert!(local.scale.abs_diff_eq(Vec3::splat(1.7), 1e-4));
    }
}
