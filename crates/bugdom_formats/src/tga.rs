//! Truevision TGA images: the textures, sprites and screens in
//! `Data/Images/**/*.tga`.
//!
//! Port of `ReadTGA` with `forceRGBA = true` (original/src/System/TGA.c).
//! We write our own decoder rather than use a general TGA library so the
//! alpha channel matches the game exactly (see the 16-bit note below).
//!
//! # Format notes
//!
//! Little-endian. An 18-byte header:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 1 | ID field length |
//! | 1 | 1 | colour-map type (1 if there is a palette) |
//! | 2 | 1 | image type: 1/2/3 uncompressed colour-mapped/true-colour/greyscale; +8 for RLE |
//! | 3 | 2 | first palette entry (must be 0) |
//! | 5 | 2 | palette entry count |
//! | 7 | 1 | bits per palette entry (must be 24) |
//! | 8 | 4 | x and y origin (ignored) |
//! | 12 | 2 | width |
//! | 14 | 2 | height |
//! | 16 | 1 | bits per pixel: 8, 16, 24 or 32 |
//! | 17 | 1 | image descriptor: bits 0–3 alpha bit count, bit 5 set if rows run top to bottom |
//!
//! Then the ID field, the palette (BGR triplets) and the pixel data. Pixels are
//! BGR(A) for 24/32 bpp, palette indices or grey levels for 8 bpp, and
//! little-endian `ARRRRRGGGGGBBBBB` words for 16 bpp. RLE data is a series of
//! packets, each a header byte (count − 1 in the low 7 bits) followed either,
//! if the top bit is set, by one pixel repeated `count` times or else by
//! `count` literal pixels. RLE works on stored pixels, i.e. palette indices
//! for colour-mapped images.
//!
//! # Quirks kept from the original
//!
//! - In 16-bit images the top bit is alpha only if bit 0 of the descriptor is
//!   set (`imageDescriptor & 1`); otherwise every pixel is opaque. Every
//!   16-bit file in the game has descriptor `0x20`, so they are all opaque
//!   even where the top bit is clear. Transparency comes from the callers
//!   instead: `QD3D_LoadTextureFile` with `kRendererTextureFlags_SolidBlackIsAlpha`
//!   and the infobar sprite masks treat pure black as transparent.
//! - 5-bit channels widen as `c * 255 / 31` (truncating), not by bit
//!   replication.
//! - Only the vertical origin bit is honoured; bit 4 (right-to-left) is
//!   ignored.
//! - Colour-mapped images must be 8 bpp with a 24-bit palette starting at
//!   entry 0; an index past the palette is an error.

use std::io::Cursor;
use std::path::Path;

use binrw::{BinRead, BinReaderExt};

use crate::error::{Error, Result, ResultExt, read_file};

/// A decoded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGBA8, rows from top to bottom.
    pub pixels: Vec<u8>,
}

impl Image {
    /// Reads and decodes a TGA file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = read_file(path)?;
        Self::parse(&bytes).context(|| format!("in {}", path.display()))
    }

    /// Decodes a TGA file held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        parse(bytes)
    }

    /// The RGBA of the pixel at column `x`, row `y` (0 is the top row).
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.pixels.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}

/// Reads and decodes a TGA file.
pub fn open(path: impl AsRef<Path>) -> Result<Image> {
    Image::open(path)
}

/// Decodes a TGA file held in memory.
pub fn parse(bytes: &[u8]) -> Result<Image> {
    let mut reader = Cursor::new(bytes);
    let header: Header = reader.read_le().context(|| "TGA header".into())?;

    let (color_mapped, compressed) = match header.image_type {
        IMAGE_TYPE_RAW_CMAP => (true, false),
        IMAGE_TYPE_RAW_BGR | IMAGE_TYPE_RAW_GRAYSCALE => (false, false),
        IMAGE_TYPE_RLE_CMAP => (true, true),
        IMAGE_TYPE_RLE_BGR | IMAGE_TYPE_RLE_GRAYSCALE => (false, true),
        other => {
            return Err(Error::invalid(format!(
                "unsupported TGA image type {other}"
            )));
        }
    };

    // The game asserts there is no ID field; skipping it is equivalent for
    // every shipped file and harmless otherwise.
    let mut rest = bytes
        .get(HEADER_LEN + usize::from(header.id_length)..)
        .ok_or_else(|| Error::invalid("ID field runs past the end"))?;

    let palette = if color_mapped {
        if header.bpp != 8 {
            return Err(Error::invalid(format!(
                "colour-mapped image with {} bpp; only 8 is supported",
                header.bpp
            )));
        }
        if header.palette_origin != 0 {
            return Err(Error::invalid("palette does not start at entry 0"));
        }
        if header.palette_count > 256 {
            return Err(Error::invalid(format!(
                "{} palette entries; at most 256 are supported",
                header.palette_count
            )));
        }
        if header.palette_bits != 24 {
            return Err(Error::invalid(format!(
                "{}-bit palette entries; only 24 is supported",
                header.palette_bits
            )));
        }
        let len = usize::from(header.palette_count) * 3;
        let palette = rest
            .get(..len)
            .ok_or_else(|| Error::invalid("palette runs past the end"))?;
        rest = &rest[len..];
        Some(palette)
    } else {
        None
    };

    let bytes_per_pixel = usize::from(header.bpp / 8);
    let pixel_count = usize::from(header.width) * usize::from(header.height);
    let data_len = pixel_count * bytes_per_pixel;

    let mut stored = if compressed {
        decompress_rle(rest, pixel_count, bytes_per_pixel).context(|| "RLE pixel data".into())?
    } else {
        rest.get(..data_len)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{data_len} bytes of pixel data expected, {} present",
                    rest.len()
                ))
            })?
            .to_vec()
    };

    if header.descriptor & DESCRIPTOR_TOP_TO_BOTTOM == 0 {
        flip_rows(&mut stored, usize::from(header.width) * bytes_per_pixel);
    }

    // Colour-mapped pixels become 24-bit BGR, as in `ConvertColormappedToBGR`.
    let (stored, bpp) = match palette {
        Some(palette) => (remap(&stored, palette)?, 24),
        None => (stored, header.bpp),
    };

    Ok(Image {
        width: u32::from(header.width),
        height: u32::from(header.height),
        pixels: to_rgba(&stored, bpp, header.descriptor)?,
    })
}

const HEADER_LEN: usize = 18;

const IMAGE_TYPE_RAW_CMAP: u8 = 1;
const IMAGE_TYPE_RAW_BGR: u8 = 2;
const IMAGE_TYPE_RAW_GRAYSCALE: u8 = 3;
const IMAGE_TYPE_RLE_CMAP: u8 = 9;
const IMAGE_TYPE_RLE_BGR: u8 = 10;
const IMAGE_TYPE_RLE_GRAYSCALE: u8 = 11;

/// Image descriptor bit: rows are stored top to bottom.
const DESCRIPTOR_TOP_TO_BOTTOM: u8 = 1 << 5;
/// Image descriptor bit the game tests to decide whether 16-bit pixels have
/// an alpha bit. Strictly, bits 0–3 hold the alpha bit count.
const DESCRIPTOR_ALPHA_BIT: u8 = 1;

#[derive(BinRead)]
#[br(little)]
struct Header {
    id_length: u8,
    _color_map_type: u8,
    image_type: u8,
    palette_origin: u16,
    palette_count: u16,
    palette_bits: u8,
    _x_origin: u16,
    _y_origin: u16,
    width: u16,
    height: u16,
    bpp: u8,
    descriptor: u8,
}

/// Port of `DecompressRLE` (TGA.c).
fn decompress_rle(mut input: &[u8], pixel_count: usize, bytes_per_pixel: usize) -> Result<Vec<u8>> {
    // A packet expands to at most 128 pixels, so a corrupt header cannot make
    // us reserve far more than the data could fill.
    let max_len = input.len().saturating_mul(128);
    let mut out = Vec::with_capacity((pixel_count * bytes_per_pixel).min(max_len));
    let mut pixels = 0;
    while pixels < pixel_count {
        let (&packet, rest) = input
            .split_first()
            .ok_or_else(|| Error::invalid("data ends before the last pixel"))?;
        input = rest;
        let count = 1 + usize::from(packet & 0x7F);
        if pixels + count > pixel_count {
            return Err(Error::invalid("a packet runs past the last pixel"));
        }
        let literal_len = if packet & 0x80 != 0 {
            bytes_per_pixel
        } else {
            count * bytes_per_pixel
        };
        let literal = input
            .get(..literal_len)
            .ok_or_else(|| Error::invalid("a packet runs past the end of the data"))?;
        input = &input[literal_len..];
        if packet & 0x80 != 0 {
            for _ in 0..count {
                out.extend_from_slice(literal);
            }
        } else {
            out.extend_from_slice(literal);
        }
        pixels += count;
    }
    Ok(out)
}

/// Port of `FlipPixelData` (TGA.c): reverses the order of the rows.
fn flip_rows(data: &mut [u8], row_bytes: usize) {
    if row_bytes == 0 {
        return;
    }
    let rows = data.len() / row_bytes;
    for top in 0..rows / 2 {
        let bottom = rows - 1 - top;
        let (upper, lower) = data.split_at_mut(bottom * row_bytes);
        upper[top * row_bytes..(top + 1) * row_bytes].swap_with_slice(&mut lower[..row_bytes]);
    }
}

/// Port of `ConvertColormappedToBGR` (TGA.c). The palette is already BGR,
/// so entries are copied as they are.
fn remap(indices: &[u8], palette: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(indices.len() * 3);
    for &index in indices {
        let i = usize::from(index) * 3;
        let color = palette.get(i..i + 3).ok_or_else(|| {
            Error::invalid(format!(
                "palette index {index} is past the {} entries",
                palette.len() / 3
            ))
        })?;
        out.extend_from_slice(color);
    }
    Ok(out)
}

/// Port of `ConvertToRGBA` (TGA.c).
fn to_rgba(data: &[u8], bpp: u8, descriptor: u8) -> Result<Vec<u8>> {
    let widen5 = |c: u16| ((c & 0x1F) * 255 / 31) as u8;
    Ok(match bpp {
        32 => data
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
        24 => data
            .chunks_exact(3)
            .flat_map(|p| [p[2], p[1], p[0], 0xFF])
            .collect(),
        16 => {
            let has_alpha = descriptor & DESCRIPTOR_ALPHA_BIT != 0;
            data.chunks_exact(2)
                .flat_map(|p| {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let a = if !has_alpha || v & 0x8000 != 0 {
                        0xFF
                    } else {
                        0x00
                    };
                    [widen5(v >> 10), widen5(v >> 5), widen5(v), a]
                })
                .collect()
        }
        8 => data.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        other => return Err(Error::invalid(format!("unsupported {other} bpp"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;
    use std::path::PathBuf;

    fn tga_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                tga_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "tga") {
                out.push(path);
            }
        }
    }

    fn image(name: &str) -> Image {
        open(original_data_dir().join("Images").join(name)).unwrap()
    }

    #[test]
    fn decodes_every_original_tga() {
        let mut files = Vec::new();
        tga_files(&original_data_dir().join("Images"), &mut files);
        assert!(files.len() >= 70, "found only {} files", files.len());
        for path in files {
            let img = Image::open(&path).unwrap_or_else(|e| panic!("{e:?}"));
            assert!(img.width > 0 && img.height > 0, "{}", path.display());
            assert_eq!(
                img.pixels.len(),
                img.width as usize * img.height as usize * 4
            );
        }
    }

    #[test]
    fn dimensions() {
        let img = image("Textures/3500.tga");
        assert_eq!((img.width, img.height), (754, 400));
        let img = image("Textures/3000.tga");
        assert_eq!((img.width, img.height), (512, 256));
        let img = image("Infobar/NitroGauge.tga");
        assert_eq!((img.width, img.height), (90, 41));
    }

    /// Finds a 16-bit file with a pixel whose alpha bit is clear and checks
    /// that the game's descriptor rule still makes it opaque.
    #[test]
    fn sixteen_bit_alpha_follows_descriptor() {
        let mut files = Vec::new();
        tga_files(&original_data_dir().join("Images"), &mut files);
        let mut checked = 0;
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            if bytes[16] != 16 {
                continue;
            }
            assert_eq!(bytes[17] & DESCRIPTOR_ALPHA_BIT, 0);
            assert_eq!(bytes[2], IMAGE_TYPE_RAW_BGR);
            let img = parse(&bytes).unwrap();
            assert!(img.pixels.chunks_exact(4).all(|p| p[3] == 0xFF));
            let data = &bytes[HEADER_LEN..];
            if data.chunks_exact(2).any(|p| p[1] & 0x80 == 0) {
                checked += 1;
            }

            // With the descriptor's alpha bit set, the same data has alpha.
            let mut with_alpha = bytes.clone();
            with_alpha[17] |= DESCRIPTOR_ALPHA_BIT;
            let img = parse(&with_alpha).unwrap();
            let w = img.width as usize;
            let h = img.height as usize;
            // The file is stored top-to-bottom (descriptor 0x20), so stored
            // pixel 0 is the top-left one.
            assert_ne!(bytes[17] & DESCRIPTOR_TOP_TO_BOTTOM, 0);
            let a = if data[1] & 0x80 != 0 { 0xFF } else { 0 };
            assert_eq!(img.pixel(0, 0).unwrap()[3], a);
            assert_eq!(img.pixels.len(), w * h * 4);
        }
        assert!(checked > 0, "no 16-bit file has a clear alpha bit");
    }

    #[test]
    fn widens_five_bit_channels_like_the_game() {
        // 0x7FFF: white without the alpha bit; 0x0421: 1/31 on each channel.
        let mut tga = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 16, 0x21];
        tga.extend_from_slice(&[0xFF, 0x7F, 0x21, 0x84]);
        let img = parse(&tga).unwrap();
        // Descriptor 0x21: top-to-bottom, with an alpha bit.
        assert_eq!(img.pixel(0, 0), Some([255, 255, 255, 0]));
        assert_eq!(img.pixel(1, 0), Some([8, 8, 8, 255]));
    }

    #[test]
    fn rle_colormapped_and_bottom_up() {
        // 1×3, RLE colour-mapped, bottom-up, palette blue/green (BGR).
        let mut tga = vec![0, 1, 9, 0, 0, 2, 0, 24, 0, 0, 0, 0, 1, 0, 3, 0, 8, 0];
        tga.extend_from_slice(&[255, 0, 0, 0, 255, 0]);
        // A run of two index-0 pixels, then one literal index-1 pixel.
        tga.extend_from_slice(&[0x81, 0, 0x00, 1]);
        let img = parse(&tga).unwrap();
        assert_eq!(img.pixel(0, 0), Some([0, 255, 0, 255])); // last stored row
        assert_eq!(img.pixel(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(img.pixel(0, 2), Some([0, 0, 255, 255]));

        // An index past the palette is rejected.
        let mut bad = tga.clone();
        *bad.last_mut().unwrap() = 2;
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn colormapped_texture_uses_bgr_palette() {
        let bytes = std::fs::read(original_data_dir().join("Images/Textures/3511.tga")).unwrap();
        let img = parse(&bytes).unwrap();
        // Bottom-up file (descriptor 0): the first stored pixel is the
        // bottom-left one. 3511 starts with a packet.
        assert_eq!(bytes[17], 0);
        let palette = &bytes[HEADER_LEN..HEADER_LEN + 256 * 3];
        let first_index = usize::from(bytes[HEADER_LEN + 256 * 3 + 1]);
        let bgr = &palette[first_index * 3..first_index * 3 + 3];
        assert_eq!(
            img.pixel(0, img.height - 1),
            Some([bgr[2], bgr[1], bgr[0], 0xFF])
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(b"short").is_err());
        // Image type 0 (no image data).
        assert!(parse(&[0; 18]).is_err());
        // Truncated uncompressed data.
        assert!(
            parse(&[
                0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 0, 24, 0, 1, 2, 3
            ])
            .is_err()
        );
        // RLE data ending early.
        assert!(
            parse(&[
                0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 0, 8, 0, 0x81, 7
            ])
            .is_err()
        );
    }
}
