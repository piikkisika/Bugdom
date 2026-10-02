//! The infobar's art: the sprites, the two bars' backgrounds and the
//! ball-time gauge's template.

use bevy::asset::UntypedAssetId;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::assets::image::TgaSettings;
use crate::assets::original_path;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_infobar_art)
        .add_systems(Update, prepare_infobar_art);
}

/// The infobar's sprites, in the order of the original's `SPRITE_*` enum
/// (original/src/Screens/Infobar.c). Sprite `n` is
/// `Images/Infobar/{128 + n}.tga`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InfobarSprite {
    Life1,
    Life2,
    Life3,
    GoldClover1,
    GoldClover2,
    GoldClover3,
    GoldClover4,
    BlueClover1,
    BlueClover2,
    BlueClover3,
    BlueClover4,
    Ladybug,
    LadybugAll,
    LadybugSmall,
    EmptyHandL,
    EmptyHandR,
    MoneyL,
    MoneyR,
    GreenKeyL,
    GreenKeyR,
    BlueKeyL,
    BlueKeyR,
    RedKeyL,
    RedKeyR,
    OrangeKeyL,
    OrangeKeyR,
    PurpleKeyL,
    PurpleKeyR,
    Ball,
    InfobarTop,
    InfobarBottom,
}

impl InfobarSprite {
    /// How many sprites there are (`MAX_SPRITES`).
    pub const COUNT: usize = Self::InfobarBottom as usize + 1;
    /// The resource number of the first sprite's file.
    const FIRST_FILE: usize = 128;

    /// The sprite's file in the original data.
    fn path(index: usize) -> String {
        format!("Images/Infobar/{}.tga", Self::FIRST_FILE + index)
    }

    /// How the sprite's image is adapted once loaded.
    ///
    /// The sprites treat pure black as transparent, as their masks do in
    /// `LoadSpriteResources`, with their edges bled for filtering. The
    /// backgrounds are drawn into the infobar texture first, which is
    /// opaque (`GL_RGB`), so their black stays black. All are clamped, as
    /// the infobar texture is (`kRendererTextureFlags_ClampBoth`).
    fn settings(index: usize) -> TgaSettings {
        let is_background = index >= Self::InfobarTop as usize;
        TgaSettings {
            black_is_transparent: !is_background,
            bleed_edges: !is_background,
            clamp: true,
        }
    }
}

/// The infobar's art, loaded once at startup as `LoadInfobarArt` loads it
/// at boot (original/src/Screens/Infobar.c).
#[derive(Resource, Debug, Clone)]
pub struct InfobarArt {
    /// One image per [`InfobarSprite`].
    sprites: Vec<Handle<Image>>,
    /// `NitroGauge.tga`: an 8-bit template whose grey value is 0 outside
    /// the gauge and 1 + an angle in degrees inside it (the loader copies
    /// it into red, green and blue).
    pub gauge_template: Handle<Image>,
    /// The sprites whose image has had its [`TgaSettings`] applied.
    prepared: HashSet<AssetId<Image>>,
}

impl InfobarArt {
    /// The image of a sprite.
    pub fn sprite(&self, sprite: InfobarSprite) -> Handle<Image> {
        self.sprites[sprite as usize].clone()
    }

    /// Every file of the art, to check that it has loaded.
    pub fn ids(&self) -> impl Iterator<Item = UntypedAssetId> + '_ {
        self.sprites
            .iter()
            .chain(std::iter::once(&self.gauge_template))
            .map(|handle| handle.id().untyped())
    }

    /// Whether every sprite has loaded and been adapted for drawing.
    pub fn is_ready(&self) -> bool {
        self.prepared.len() == self.sprites.len()
    }
}

/// Port of `LoadInfobarArt` (original/src/Screens/Infobar.c).
fn load_infobar_art(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(InfobarArt {
        sprites: (0..InfobarSprite::COUNT)
            .map(|index| assets.load(original_path(&InfobarSprite::path(index))))
            .collect(),
        gauge_template: assets.load(original_path("Images/Infobar/NitroGauge.tga")),
        prepared: HashSet::default(),
    });
}

/// Applies each sprite's [`TgaSettings`] once its image has loaded: the
/// masks `LoadSpriteResources` builds, and the clamping of the infobar
/// texture.
fn prepare_infobar_art(art: Option<ResMut<InfobarArt>>, mut images: ResMut<Assets<Image>>) {
    let Some(mut art) = art else {
        return;
    };
    if art.is_ready() {
        return;
    }
    let art = &mut *art;
    for (index, handle) in art.sprites.iter().enumerate() {
        if art.prepared.contains(&handle.id()) {
            continue;
        }
        if let Some(mut image) = images.get_mut(handle) {
            InfobarSprite::settings(index).apply(&mut image);
            art.prepared.insert(handle.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_follow_the_original_numbering() {
        assert_eq!(InfobarSprite::COUNT, 31);
        assert_eq!(
            InfobarSprite::path(InfobarSprite::Ball as usize),
            "Images/Infobar/156.tga"
        );
        assert_eq!(
            InfobarSprite::path(InfobarSprite::InfobarTop as usize),
            "Images/Infobar/157.tga"
        );
        assert_eq!(
            InfobarSprite::path(InfobarSprite::InfobarBottom as usize),
            "Images/Infobar/158.tga"
        );
    }

    #[test]
    fn every_sprite_file_exists() {
        let dir = bugdom_formats::original_data_dir();
        for index in 0..InfobarSprite::COUNT {
            let path = dir.join(InfobarSprite::path(index));
            assert!(path.is_file(), "{} is missing", path.display());
        }
        assert!(dir.join("Images/Infobar/NitroGauge.tga").is_file());
    }

    #[test]
    fn only_the_backgrounds_keep_their_black() {
        let sprite = InfobarSprite::settings(InfobarSprite::LadybugSmall as usize);
        assert!(sprite.black_is_transparent && sprite.bleed_edges && sprite.clamp);
        let background = InfobarSprite::settings(InfobarSprite::InfobarBottom as usize);
        assert!(!background.black_is_transparent && !background.bleed_edges);
        assert!(background.clamp);
    }
}
