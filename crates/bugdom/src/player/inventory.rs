//! What the player carries: keys, money, ladybugs, clovers and lives.
//!
//! Port of the inventory half of original/src/Screens/Infobar.c. Ball
//! time is [`BallTime`](super::BallTime), and the shield is
//! [`ShieldTimer`](super::ShieldTimer).

use bevy::prelude::*;

/// Lives at the start of a game (`InitInventoryForGame`).
pub const STARTING_LIVES: u8 = 3;

/// The door keys (`MAX_KEY_TYPES`), in the order of their numbers and of
/// the infobar's key sprites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DoorKey {
    Green,
    Blue,
    Red,
    Orange,
    Purple,
}

impl DoorKey {
    pub const ALL: [Self; 5] = [
        Self::Green,
        Self::Blue,
        Self::Red,
        Self::Orange,
        Self::Purple,
    ];

    /// The key with a map item's key number, if it is one.
    pub fn from_number(number: u8) -> Option<Self> {
        Self::ALL.get(usize::from(number)).copied()
    }
}

/// What the bug holds in one of the hands the infobar shows
/// (`gLeftArmType`, `gRightArmType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HandItem {
    #[default]
    Empty,
    Money,
    Key(DoorKey),
}

/// The player's inventory. It starts empty at each level for now; keeping
/// lives and gold clovers between levels arrives with the game flow.
///
/// Port of the inventory globals of original/src/Screens/Infobar.c.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    keys: [bool; DoorKey::ALL.len()],
    /// Coins for the water bug taxi (`gMoney`).
    pub money: u16,
    /// Ladybugs freed in this area (`gNumLadyBugsThisArea`).
    pub ladybugs: u16,
    /// Ladybugs there are to free in this area (`gNumLadyBugsOnThisLevel`).
    pub ladybugs_in_area: u16,
    pub green_clovers: u16,
    pub gold_clovers: u16,
    pub blue_clovers: u16,
    /// Lives left, including the one being played (`gNumLives`). Running
    /// out does nothing yet: the game over arrives with the game flow.
    pub lives: u8,
    /// What the left and right hands hold.
    pub hands: [HandItem; 2],
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            keys: [false; DoorKey::ALL.len()],
            money: 0,
            ladybugs: 0,
            ladybugs_in_area: 0,
            green_clovers: 0,
            gold_clovers: 0,
            blue_clovers: 0,
            lives: STARTING_LIVES,
            hands: [HandItem::Empty; 2],
        }
    }
}

impl Inventory {
    /// The inventory at the start of an area with this many ladybugs to
    /// free. Port of `InitInventoryForArea` and, for now,
    /// `InitInventoryForGame`.
    pub fn for_area(ladybugs_in_area: u16) -> Self {
        Self {
            ladybugs_in_area,
            ..default()
        }
    }

    /// Port of `GetKey`.
    pub fn get_key(&mut self, key: DoorKey) {
        self.keys[key as usize] = true;
        self.hold(HandItem::Key(key));
    }

    /// Takes a key out of the inventory when it opens a door. Port of
    /// `UseKey`.
    pub fn use_key(&mut self, key: DoorKey) {
        self.keys[key as usize] = false;
        self.let_go(HandItem::Key(key));
    }

    /// Port of `DoWeHaveTheKey`.
    pub fn has_key(&self, key: DoorKey) -> bool {
        self.keys[key as usize]
    }

    /// Port of `GetMoney`.
    pub fn get_money(&mut self) {
        self.money += 1;
        self.hold(HandItem::Money);
    }

    /// Pays a coin for a taxi ride. Port of `UseMoney`, which never checks
    /// for an empty purse; this stops at zero.
    pub fn use_money(&mut self) {
        self.money = self.money.saturating_sub(1);
        self.let_go(HandItem::Money);
    }

    /// Port of `DoWeHaveEnoughMoney`.
    pub fn has_enough_money(&self) -> bool {
        self.money > 0
    }

    /// Port of `GetLadyBug`.
    pub fn get_ladybug(&mut self) {
        self.ladybugs += 1;
    }

    /// Whether every ladybug in the area is free (the infobar's
    /// `SPRITE_LADYBUG_ALL` in `ShowLadyBugs`).
    pub fn has_all_ladybugs(&self) -> bool {
        self.ladybugs >= self.ladybugs_in_area
    }

    /// Port of `GetGreenClover`.
    pub fn get_green_clover(&mut self) {
        self.green_clovers += 1;
    }

    /// Port of `GetGoldClover`.
    pub fn get_gold_clover(&mut self) {
        self.gold_clovers += 1;
    }

    /// Port of `GetBlueClover`.
    pub fn get_blue_clover(&mut self) {
        self.blue_clovers += 1;
    }

    /// A free life (the `gNumLives++` of the lives powerup).
    pub fn get_life(&mut self) {
        self.lives = self.lives.saturating_add(1);
    }

    /// Port of the `gNumLives--` in `DoDeathReset`
    /// (original/src/System/Main.c).
    pub fn lose_life(&mut self) {
        self.lives = self.lives.saturating_sub(1);
    }

    /// Puts an item in the left hand if it is empty, else in the right if
    /// that is; with both full it isn't shown.
    fn hold(&mut self, item: HandItem) {
        if let Some(hand) = self.hands.iter_mut().find(|h| **h == HandItem::Empty) {
            *hand = item;
        }
    }

    /// Empties the left hand if it holds the item, else the right.
    fn let_go(&mut self, item: HandItem) {
        if let Some(hand) = self.hands.iter_mut().find(|h| **h == item) {
            *hand = HandItem::Empty;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_kept_until_used() {
        let mut inventory = Inventory::default();
        assert!(!inventory.has_key(DoorKey::Red));
        inventory.get_key(DoorKey::Red);
        assert!(inventory.has_key(DoorKey::Red));
        assert!(!inventory.has_key(DoorKey::Blue));
        inventory.use_key(DoorKey::Red);
        assert!(!inventory.has_key(DoorKey::Red));
        assert_eq!(DoorKey::from_number(4), Some(DoorKey::Purple));
        assert_eq!(DoorKey::from_number(5), None);
    }

    #[test]
    fn money_is_counted_and_spent() {
        let mut inventory = Inventory::default();
        assert!(!inventory.has_enough_money());
        inventory.get_money();
        inventory.get_money();
        assert_eq!(inventory.money, 2);
        inventory.use_money();
        assert!(inventory.has_enough_money());
        inventory.use_money();
        assert!(!inventory.has_enough_money());
        inventory.use_money();
        assert_eq!(inventory.money, 0);
    }

    #[test]
    fn the_hands_fill_left_first_and_empty_left_first() {
        let mut inventory = Inventory::default();
        inventory.get_money();
        inventory.get_key(DoorKey::Green);
        inventory.get_key(DoorKey::Blue);
        assert_eq!(
            inventory.hands,
            [HandItem::Money, HandItem::Key(DoorKey::Green)]
        );
        // The blue key is held but not shown.
        assert!(inventory.has_key(DoorKey::Blue));
        inventory.use_money();
        assert_eq!(
            inventory.hands,
            [HandItem::Empty, HandItem::Key(DoorKey::Green)]
        );
        inventory.use_key(DoorKey::Green);
        assert_eq!(inventory.hands, [HandItem::Empty; 2]);
    }

    #[test]
    fn ladybugs_and_clovers_are_counted() {
        let mut inventory = Inventory::for_area(2);
        inventory.get_ladybug();
        assert!(!inventory.has_all_ladybugs());
        inventory.get_ladybug();
        assert!(inventory.has_all_ladybugs());
        inventory.get_green_clover();
        inventory.get_gold_clover();
        inventory.get_gold_clover();
        inventory.get_blue_clover();
        assert_eq!(
            (
                inventory.green_clovers,
                inventory.gold_clovers,
                inventory.blue_clovers
            ),
            (1, 2, 1)
        );
    }

    #[test]
    fn lives_start_at_three_and_stop_at_zero() {
        let mut inventory = Inventory::default();
        assert_eq!(inventory.lives, STARTING_LIVES);
        inventory.get_life();
        assert_eq!(inventory.lives, 4);
        for _ in 0..10 {
            inventory.lose_life();
        }
        assert_eq!(inventory.lives, 0);
    }
}
