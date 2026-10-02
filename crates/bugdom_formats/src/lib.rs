//! Parsers for the original Bugdom data formats: resource forks, 3DMF models,
//! skeletons, terrain, sound banks and images.
//!
//! This crate deliberately has no Bevy dependency, so the converter and tests
//! can use it without pulling in the engine.

pub mod aiff;
mod error;
mod four_cc;
pub mod mac_roman;
pub mod rsrc;
pub mod skeleton;
pub mod skin;
pub mod tdmf;
pub mod terrain;
pub mod tga;

pub use error::{Error, Result, ResultExt};
pub use four_cc::FourCC;

use std::path::PathBuf;

/// The original game data in the repository's `original/` submodule.
///
/// Only valid when building from a checkout of this repository. It is meant
/// for tests and development builds, not for locating data at runtime in a
/// packaged game.
pub fn original_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../original/Data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_data_is_present() {
        let dir = original_data_dir();
        for sub in ["Audio", "Images", "Models", "Skeletons", "Terrain"] {
            assert!(
                dir.join(sub).is_dir(),
                "{} is missing; run `git submodule update --init --recursive`",
                dir.join(sub).display()
            );
        }
    }
}
