use std::fmt;

use binrw::BinRead;

/// A four-character code (Mac `OSType`), used for resource types, 3DMF chunk
/// types and AIFF chunk IDs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BinRead)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    pub const fn new(code: &[u8; 4]) -> Self {
        Self(*code)
    }
}

impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::mac_roman::decode(&self.0))
    }
}

impl fmt::Debug for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{self}'")
    }
}
