//! AIFF and AIFF-C audio files: the music in `Data/Audio/*.aiff` and the sound
//! effects in the `Data/Audio/*.sounds/` banks (a bank is just a directory).
//!
//! Port of `GetSoundInfoFromAIFF` (original/extern/Pomme/src/SoundFormats/AIFF.cpp),
//! the IMA4 decoder (`IMA4.cpp`, itself adapted from FFmpeg's `adpcm.c`), the
//! µ-law/A-law decoder (`xlaw.cpp`), the PCM handling in
//! `Pomme_DecompressSoundResource` (`SoundFormats.cpp`) and `WavStream::FillBuffer`
//! (`SoundMixer/cmixer.cpp`), and `ConvertFromIeeeExtended`
//! (`Utilities/IEEEExtended.cpp`).
//!
//! # Format notes
//!
//! Everything is big-endian. A file is an IFF `FORM` chunk (`'FORM'`, `u32`
//! size, form type `'AIFF'` or `'AIFC'`) holding sub-chunks, each an ID, a
//! `u32` size and the data, padded to an even length. The chunks we use:
//!
//! - `COMM`: `u16` channel count, `u32` frame count (for compressed data, the
//!   number of *packets*), `u16` bits per sample, the sample rate as an 80-bit
//!   IEEE 754 extended float, then in AIFF-C only the compression type
//!   (four-char code) and a Pascal-string name padded to an even length.
//! - `SSND`: `u32` offset and `u32` block size (both 0 in practice), then the
//!   sample data, channels interleaved per frame.
//! - `MARK`: `u16` count, then per marker a `u16` ID, a `u32` position in
//!   (decoded) sample frames and an even-padded Pascal-string name.
//! - `INST`: base note (MIDI, signed byte), detune, note and velocity ranges,
//!   `i16` gain, then the sustain loop and the release loop, each a `u16`
//!   play mode (0 none, 1 forward, 2 forward/backward) and `u16` begin and end
//!   marker IDs.
//!
//! Others (`FVER`, `NAME`, `ANNO`, `APPL`, `COMT`) are skipped.
//!
//! Compression types used by the game: `ima4` (most files), `twos` (signed
//! big-endian PCM), `ulaw` and `raw ` (unsigned 8-bit PCM). We also accept
//! `NONE` (plain AIFF), `sowt` (little-endian PCM) and `alaw`, which Pomme
//! supports too. Pomme's `MAC3` (MACE 3:1) is not ported, as no file uses it.
//!
//! IMA4 is Apple's QuickTime flavour of IMA ADPCM: each packet is 34 bytes per
//! channel (a 2-byte header plus 32 bytes of 4-bit codes) and decodes to 64
//! frames. Packets for each channel follow one another; a stereo frame of
//! packets is left then right.
//!
//! # Playback semantics
//!
//! The game plays effects at a pitch derived from [`Sound::base_note`] (see
//! `PlayEffect_Parms` in original/src/System/Sound.c, which sends `freqCmd`).
//! Pomme loops a sound when its sustain loop spans at least two frames: playback
//! restarts at the loop start after reaching the *end of the sound*; Pomme
//! ignores the loop end marker (see `InstallSoundInChannel` in
//! original/extern/Pomme/src/SoundMixer/SoundManager.cpp). Songs have no
//! sustain loop; `PlaySong` makes them loop through a separate flag.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use binrw::{BinRead, BinReaderExt};

use crate::error::{Error, Result, ResultExt, read_file};
use crate::four_cc::FourCC;

/// A decoded sound.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub channels: u16,
    /// Frames per second. Stored as an 80-bit float, so not necessarily an
    /// integer, though every shipped file's rate is (`Main.sounds/Select.aiff`
    /// is the odd one out at 22257 Hz).
    pub sample_rate: f64,
    /// Signed 16-bit PCM, channels interleaved per frame.
    pub samples: Vec<i16>,
    /// The compression type the file was stored with, for diagnostics.
    pub compression: FourCC,
    /// The MIDI note at which the sound plays at its recorded rate. Defaults
    /// to 60 (middle C) when there is no `INST` chunk.
    pub base_note: i8,
    /// The `INST` sustain loop, if its play mode is "forward".
    pub sustain_loop: Option<Loop>,
}

/// A loop, in sample frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loop {
    pub start: u32,
    pub end: u32,
}

impl Sound {
    /// Reads and decodes an AIFF or AIFF-C file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = read_file(path)?;
        Self::parse(&bytes).context(|| format!("in {}", path.display()))
    }

    /// Decodes an AIFF or AIFF-C file held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        parse(bytes)
    }

    /// The number of sample frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// The duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate
    }

    /// Whether Pomme would loop this sound: it does so when the sustain loop
    /// spans at least two frames.
    pub fn loops(&self) -> bool {
        self.sustain_loop
            .is_some_and(|l| l.end.saturating_sub(l.start) >= 2)
    }
}

/// Reads and decodes an AIFF or AIFF-C file.
pub fn open(path: impl AsRef<Path>) -> Result<Sound> {
    Sound::open(path)
}

/// Decodes an AIFF or AIFF-C file held in memory.
pub fn parse(bytes: &[u8]) -> Result<Sound> {
    let mut header = Cursor::new(bytes);
    let form: FormHeader = header.read_be().context(|| "FORM header".into())?;
    let is_aifc = match &form.form_type.0 {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => {
            return Err(Error::invalid(format!(
                "form type {:?} is neither AIFF nor AIFC",
                form.form_type
            )));
        }
    };
    let form_end = (FORM_HEADER_LEN as u64 - 4 + u64::from(form.size)).min(bytes.len() as u64);
    let body = bytes
        .get(FORM_HEADER_LEN..form_end as usize)
        .ok_or_else(|| Error::invalid("FORM is shorter than its header"))?;

    let mut comm = None;
    let mut ssnd = None;
    let mut inst = None;
    let mut markers = BTreeMap::new();

    for chunk in chunks(body) {
        let (id, data) = chunk?;
        match &id.0 {
            b"COMM" => comm = Some(parse_comm(data, is_aifc).context(|| "COMM chunk".into())?),
            b"SSND" => ssnd = Some(parse_ssnd(data).context(|| "SSND chunk".into())?),
            b"MARK" => parse_mark(data, &mut markers).context(|| "MARK chunk".into())?,
            b"INST" => inst = Some(Cursor::new(data).read_be::<Inst>()?),
            _ => {}
        }
    }

    let comm = comm.ok_or_else(|| Error::invalid("no COMM chunk"))?;
    let ssnd = ssnd.ok_or_else(|| Error::invalid("no SSND chunk"))?;
    let samples = decode(&comm, ssnd).context(|| format!("{:?} sample data", comm.compression))?;

    // Pomme resolves markers while reading `INST`, so it requires `MARK` to
    // come first. We resolve them afterwards, which also accepts the
    // opposite order; the shipped files with a looping `INST` all have `MARK`
    // first, so this changes nothing for them.
    let (base_note, sustain_loop) = match inst {
        None => (DEFAULT_BASE_NOTE, None),
        Some(inst) => {
            let sustain_loop = match inst.sustain_play_mode {
                PLAY_MODE_NONE => None,
                PLAY_MODE_FORWARD => {
                    let marker = |id: u16| {
                        markers.get(&id).copied().ok_or_else(|| {
                            Error::invalid(format!("INST refers to missing marker {id}"))
                        })
                    };
                    Some(Loop {
                        start: marker(inst.sustain_begin)?,
                        end: marker(inst.sustain_end)?,
                    })
                }
                mode => {
                    return Err(Error::invalid(format!(
                        "unsupported INST sustain loop play mode {mode}"
                    )));
                }
            };
            (inst.base_note, sustain_loop)
        }
    };

    Ok(Sound {
        channels: comm.channels,
        sample_rate: comm.sample_rate,
        samples,
        compression: comm.compression,
        base_note,
        sustain_loop,
    })
}

/// Size of `'FORM'`, the form size and the form type.
const FORM_HEADER_LEN: usize = 12;
/// Middle C, Pomme's default when there is no `INST` chunk.
const DEFAULT_BASE_NOTE: i8 = 60;
const PLAY_MODE_NONE: u16 = 0;
const PLAY_MODE_FORWARD: u16 = 1;

#[derive(BinRead)]
#[br(big, magic = b"FORM")]
struct FormHeader {
    size: u32,
    form_type: FourCC,
}

#[derive(BinRead)]
#[br(big)]
struct ChunkHeader {
    id: FourCC,
    size: u32,
}

/// Iterates over the `(id, data)` of the chunks in a FORM body.
fn chunks(mut body: &[u8]) -> impl Iterator<Item = Result<(FourCC, &[u8])>> {
    std::iter::from_fn(move || {
        if body.is_empty() {
            return None;
        }
        match split_chunk(body) {
            Ok((id, data, rest)) => {
                body = rest;
                Some(Ok((id, data)))
            }
            Err(e) => {
                body = &[];
                Some(Err(e))
            }
        }
    })
}

/// Splits the first chunk off a FORM body: its ID, its data and what follows.
fn split_chunk(body: &[u8]) -> Result<(FourCC, &[u8], &[u8])> {
    let header: ChunkHeader = Cursor::new(body).read_be()?;
    let start: usize = 8;
    let data = start
        .checked_add(header.size as usize)
        .and_then(|end| body.get(start..end))
        .ok_or_else(|| {
            Error::invalid(format!(
                "chunk {:?} of {} bytes runs past the end of the FORM",
                header.id, header.size
            ))
        })?;
    // Chunks are padded to an even length; a missing pad byte at the very
    // end is harmless.
    let end = start + data.len();
    let next = (end + (end & 1)).min(body.len());
    Ok((header.id, data, &body[next..]))
}

struct Comm {
    channels: u16,
    /// Frames, or packets for compressed data.
    packets: u32,
    bits_per_sample: u16,
    sample_rate: f64,
    compression: FourCC,
}

#[derive(BinRead)]
#[br(big)]
struct CommFixed {
    channels: u16,
    packets: u32,
    bits_per_sample: u16,
    sample_rate: [u8; 10],
}

/// Port of `ParseCOMM` (original/extern/Pomme/src/SoundFormats/AIFF.cpp).
fn parse_comm(data: &[u8], is_aifc: bool) -> Result<Comm> {
    let mut reader = Cursor::new(data);
    let fixed: CommFixed = reader.read_be()?;
    // The compression name that follows is only for humans.
    let compression = if is_aifc {
        reader.read_be()?
    } else {
        FourCC::new(b"NONE")
    };
    if fixed.channels == 0 {
        return Err(Error::invalid("zero channels"));
    }
    Ok(Comm {
        channels: fixed.channels,
        packets: fixed.packets,
        bits_per_sample: fixed.bits_per_sample,
        sample_rate: extended_to_f64(&fixed.sample_rate),
        compression,
    })
}

/// Returns the sample data of an `SSND` chunk.
fn parse_ssnd(data: &[u8]) -> Result<&[u8]> {
    let (offset, _block_size): (u32, u32) = Cursor::new(data).read_be()?;
    // Pomme requires the offset to be 0; skipping it is what the spec asks
    // for and is the same for every shipped file.
    data.get(8 + offset as usize..)
        .ok_or_else(|| Error::invalid(format!("data offset {offset} is past the end")))
}

/// Port of `ParseMARK` (original/extern/Pomme/src/SoundFormats/AIFF.cpp).
fn parse_mark(data: &[u8], markers: &mut BTreeMap<u16, u32>) -> Result<()> {
    let mut reader = Cursor::new(data);
    let count: i16 = reader.read_be()?;
    for _ in 0..count.max(0) {
        let (id, position, name_len): (u16, u32, u8) = reader.read_be()?;
        // The name is a Pascal string padded so that length byte plus text
        // is even.
        let name_len = u64::from(name_len);
        let skip = name_len + (name_len + 1) % 2;
        reader.set_position(reader.position() + skip);
        markers.insert(id, position);
    }
    Ok(())
}

#[derive(BinRead)]
#[br(big)]
struct Inst {
    base_note: i8,
    _detune: i8,
    _low_note: u8,
    _high_note: u8,
    _low_velocity: u8,
    _high_velocity: u8,
    _gain: i16,
    sustain_play_mode: u16,
    sustain_begin: u16,
    sustain_end: u16,
    // The release loop is ignored, as in Pomme.
    _release_loop: [u16; 3],
}

/// Decodes the sample data to interleaved signed 16-bit PCM.
fn decode(comm: &Comm, data: &[u8]) -> Result<Vec<i16>> {
    let channels = usize::from(comm.channels);
    let packets = comm.packets as usize;
    match &comm.compression.0 {
        b"ima4" => {
            let expected = packets * IMA4_PACKET_BYTES * channels;
            check_len(data, expected)?;
            decode_ima4(&data[..expected], channels)
        }
        b"ulaw" | b"alaw" => {
            let expected = packets * channels;
            check_len(data, expected)?;
            let table = if comm.compression.0 == *b"ulaw" {
                &ULAW_TO_PCM
            } else {
                &ALAW_TO_PCM
            };
            Ok(decode_xlaw(&data[..expected], table))
        }
        b"NONE" | b"twos" | b"sowt" | b"raw " => decode_pcm(comm, data),
        _ => Err(Error::invalid("unsupported AIFF-C compression type")),
    }
}

/// Pomme refuses compressed data whose size does not match the `COMM` packet
/// count; so do we, but trailing bytes are tolerated.
fn check_len(data: &[u8], expected: usize) -> Result<()> {
    if data.len() < expected {
        return Err(Error::invalid(format!(
            "{} bytes of sample data, COMM implies {expected}",
            data.len()
        )));
    }
    Ok(())
}

/// Uncompressed PCM. Port of the PCM paths in `Pomme_DecompressSoundResource`
/// (SoundFormats.cpp) and `WavStream::FillBuffer` (SoundMixer/cmixer.cpp).
///
/// Like Pomme, the frame count comes from the `SSND` size rather than `COMM`.
/// Pomme's mixer treats all 8-bit data as unsigned (offset by 128), which is
/// right for `raw ` but not for `NONE`/`twos`/`sowt`, which the AIFF spec
/// defines as signed; we decode those as signed. No shipped file is 8-bit
/// signed, so this changes nothing in practice.
fn decode_pcm(comm: &Comm, data: &[u8]) -> Result<Vec<i16>> {
    let unsigned = comm.compression.0 == *b"raw ";
    let little_endian = comm.compression.0 == *b"sowt";
    let bytes_per_sample = match comm.bits_per_sample {
        8 => 1,
        16 => 2,
        bits => return Err(Error::invalid(format!("unsupported {bits}-bit PCM"))),
    };
    let frame_bytes = bytes_per_sample * usize::from(comm.channels);
    if !data.len().is_multiple_of(frame_bytes) {
        return Err(Error::invalid(format!(
            "{} bytes of sample data is not a whole number of {frame_bytes}-byte frames",
            data.len()
        )));
    }
    Ok(match (bytes_per_sample, unsigned, little_endian) {
        (1, true, _) => data.iter().map(|&b| (i16::from(b) - 128) << 8).collect(),
        (1, false, _) => data.iter().map(|&b| i16::from(b as i8) << 8).collect(),
        (_, _, true) => data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect(),
        (_, _, false) => data
            .chunks_exact(2)
            .map(|b| i16::from_be_bytes([b[0], b[1]]))
            .collect(),
    })
}

/// Bytes per channel in one IMA4 packet: a 2-byte header and 32 bytes of codes.
const IMA4_PACKET_BYTES: usize = 34;
/// Frames decoded from one IMA4 packet.
const IMA4_FRAMES_PER_PACKET: usize = 64;

const IMA_INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const IMA_STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// Decoder state of one channel; persists from packet to packet.
#[derive(Default, Clone, Copy)]
struct ImaChannel {
    predictor: i32,
    step_index: i32,
}

impl ImaChannel {
    /// Port of `adpcm_ima_qt_expand_nibble` (IMA4.cpp).
    fn expand_nibble(&mut self, nibble: u8) -> i16 {
        let step = i32::from(IMA_STEP_TABLE[self.step_index as usize]);
        let step_index =
            (self.step_index + i32::from(IMA_INDEX_TABLE[usize::from(nibble)])).clamp(0, 88);

        let mut diff = step >> 3;
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        let predictor = if nibble & 8 != 0 {
            self.predictor - diff
        } else {
            self.predictor + diff
        };

        self.predictor = predictor.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        self.step_index = step_index;
        self.predictor as i16
    }
}

/// Port of `IMA4::Decode` and `DecodeIMA4Chunk` (IMA4.cpp). `data` must hold a
/// whole number of packet groups (34 bytes × channels).
fn decode_ima4(data: &[u8], channels: usize) -> Result<Vec<i16>> {
    let groups = data.chunks_exact(IMA4_PACKET_BYTES * channels);
    let mut out = vec![0; groups.len() * IMA4_FRAMES_PER_PACKET * channels];
    let mut state = vec![ImaChannel::default(); channels];

    for (group, out) in groups.zip(out.chunks_exact_mut(IMA4_FRAMES_PER_PACKET * channels)) {
        for (chan, (packet, cs)) in group
            .chunks_exact(IMA4_PACKET_BYTES)
            .zip(state.iter_mut())
            .enumerate()
        {
            // The header holds the top 9 bits of the initial predictor and,
            // in the low 7 bits, the step index.
            let header = i32::from(i16::from_be_bytes([packet[0], packet[1]]));
            let step_index = header & 0x7F;
            let predictor = header & !0x7F;

            // Like FFmpeg, keep the running predictor (which has more
            // precision than the header) unless the header disagrees.
            if cs.step_index != step_index || (predictor - cs.predictor).abs() > 0x7F {
                cs.step_index = step_index;
                cs.predictor = predictor;
            }
            if cs.step_index > 88 {
                return Err(Error::invalid(format!(
                    "IMA4 step index {} is past the end of the step table",
                    cs.step_index
                )));
            }

            for (m, &byte) in packet[2..].iter().enumerate() {
                out[(2 * m) * channels + chan] = cs.expand_nibble(byte & 0x0F);
                out[(2 * m + 1) * channels + chan] = cs.expand_nibble(byte >> 4);
            }
        }
    }
    Ok(out)
}

/// Port of `xlaw::Decode` (xlaw.cpp). The tables cover non-negative input
/// bytes (as `i8`); negative ones mirror the table and negate the output.
fn decode_xlaw(data: &[u8], table: &[i16; 128]) -> Vec<i16> {
    data.iter()
        .map(|&b| {
            let b = b as i8;
            if b < 0 {
                table[(128 + i16::from(b)) as usize].wrapping_neg()
            } else {
                table[b as usize]
            }
        })
        .collect()
}

#[rustfmt::skip]
const ALAW_TO_PCM: [i16; 128] = [
     -5504,  -5248,  -6016,  -5760,  -4480,  -4224,  -4992,  -4736,
     -7552,  -7296,  -8064,  -7808,  -6528,  -6272,  -7040,  -6784,
     -2752,  -2624,  -3008,  -2880,  -2240,  -2112,  -2496,  -2368,
     -3776,  -3648,  -4032,  -3904,  -3264,  -3136,  -3520,  -3392,
    -22016, -20992, -24064, -23040, -17920, -16896, -19968, -18944,
    -30208, -29184, -32256, -31232, -26112, -25088, -28160, -27136,
    -11008, -10496, -12032, -11520,  -8960,  -8448,  -9984,  -9472,
    -15104, -14592, -16128, -15616, -13056, -12544, -14080, -13568,
      -344,   -328,   -376,   -360,   -280,   -264,   -312,   -296,
      -472,   -456,   -504,   -488,   -408,   -392,   -440,   -424,
       -88,    -72,   -120,   -104,    -24,     -8,    -56,    -40,
      -216,   -200,   -248,   -232,   -152,   -136,   -184,   -168,
     -1376,  -1312,  -1504,  -1440,  -1120,  -1056,  -1248,  -1184,
     -1888,  -1824,  -2016,  -1952,  -1632,  -1568,  -1760,  -1696,
      -688,   -656,   -752,   -720,   -560,   -528,   -624,   -592,
      -944,   -912,  -1008,   -976,   -816,   -784,   -880,   -848,
];

#[rustfmt::skip]
const ULAW_TO_PCM: [i16; 128] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956,
    -23932, -22908, -21884, -20860, -19836, -18812, -17788, -16764,
    -15996, -15484, -14972, -14460, -13948, -13436, -12924, -12412,
    -11900, -11388, -10876, -10364,  -9852,  -9340,  -8828,  -8316,
     -7932,  -7676,  -7420,  -7164,  -6908,  -6652,  -6396,  -6140,
     -5884,  -5628,  -5372,  -5116,  -4860,  -4604,  -4348,  -4092,
     -3900,  -3772,  -3644,  -3516,  -3388,  -3260,  -3132,  -3004,
     -2876,  -2748,  -2620,  -2492,  -2364,  -2236,  -2108,  -1980,
     -1884,  -1820,  -1756,  -1692,  -1628,  -1564,  -1500,  -1436,
     -1372,  -1308,  -1244,  -1180,  -1116,  -1052,   -988,   -924,
      -876,   -844,   -812,   -780,   -748,   -716,   -684,   -652,
      -620,   -588,   -556,   -524,   -492,   -460,   -428,   -396,
      -372,   -356,   -340,   -324,   -308,   -292,   -276,   -260,
      -244,   -228,   -212,   -196,   -180,   -164,   -148,   -132,
      -120,   -112,   -104,    -96,    -88,    -80,    -72,    -64,
       -56,    -48,    -40,    -32,    -24,    -16,     -8,      0,
];

/// Converts an 80-bit IEEE 754 extended float (big-endian: sign and 15-bit
/// exponent, then a 64-bit mantissa with an explicit integer bit).
///
/// Port of `ConvertFromIeeeExtended` (original/extern/Pomme/src/Utilities/IEEEExtended.cpp),
/// including its mapping of infinities and NaNs to infinity.
fn extended_to_f64(bytes: &[u8; 10]) -> f64 {
    let exponent = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7FFF);
    let hi = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let lo = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);

    let magnitude = if exponent == 0 && hi == 0 && lo == 0 {
        0.0
    } else if exponent == 0x7FFF {
        f64::INFINITY
    } else {
        let exponent = exponent - 16383;
        f64::from(hi) * pow2(exponent - 31) + f64::from(lo) * pow2(exponent - 63)
    };

    if bytes[0] & 0x80 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// `2^n` as an `f64`, like `ldexp(1.0, n)`.
fn pow2(n: i32) -> f64 {
    // Split the exponent so that each factor stays a normal, exact power of
    // two; their product then rounds only once, at the very end.
    let half = n / 2;
    let normal = |e: i32| f64::from_bits(((e.clamp(-1022, 1023) + 1023) as u64) << 52);
    normal(half) * normal(n - half)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_data_dir;
    use std::path::PathBuf;

    fn aiff_files() -> Vec<PathBuf> {
        let audio = original_data_dir().join("Audio");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&audio).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                for e in std::fs::read_dir(&path).unwrap() {
                    files.push(e.unwrap().path());
                }
            } else {
                files.push(path);
            }
        }
        files.retain(|p| p.extension().is_some_and(|e| e == "aiff"));
        files.sort();
        files
    }

    fn sound(name: &str) -> Sound {
        open(original_data_dir().join("Audio").join(name)).unwrap()
    }

    #[test]
    fn decodes_every_original_aiff() {
        let files = aiff_files();
        assert!(files.len() >= 50, "found only {} files", files.len());
        for path in files {
            let s = Sound::open(&path).unwrap_or_else(|e| panic!("{e:?}"));
            assert!(s.frames() > 0, "{}", path.display());
            assert_eq!(s.samples.len() % usize::from(s.channels), 0);
            assert!(
                [11025.0, 22050.0, 22257.0, 44100.0].contains(&s.sample_rate),
                "{}: {} Hz",
                path.display(),
                s.sample_rate
            );
            if let Some(l) = s.sustain_loop {
                assert!(l.start <= l.end && l.end as usize <= s.frames());
            }
        }
    }

    #[test]
    fn frame_counts_match_comm() {
        // COMM counts packets of 64 frames for ima4 and frames otherwise.
        let song = sound("Forest.aiff");
        assert_eq!((song.channels, song.sample_rate), (2, 44100.0));
        assert_eq!(song.compression, FourCC::new(b"ima4"));
        assert_eq!(song.frames(), 87824 * 64);

        let clang = sound("AntHill.sounds/PipeClang.aiff");
        assert_eq!((clang.channels, clang.sample_rate), (1, 22050.0));
        assert_eq!(clang.compression, FourCC::new(b"twos"));
        assert_eq!(clang.frames(), 17024);

        let valve = sound("AntHill.sounds/ValveOpen.aiff");
        assert_eq!(valve.compression, FourCC::new(b"ulaw"));
        assert_eq!(valve.frames(), 17024);

        let select = sound("Main.sounds/Select.aiff");
        assert_eq!(select.compression, FourCC::new(b"raw "));
        assert_eq!(select.frames(), 2645);
        // 0x400D ADE2 0000 0000 0000: 0xADE2 × 2^(14 − 15).
        assert_eq!(select.sample_rate, 22257.0);
    }

    #[test]
    fn loops_and_base_notes() {
        let pump = sound("Hive.sounds/Pump.aiff");
        assert_eq!(
            pump.sustain_loop,
            Some(Loop {
                start: 0,
                end: 46769
            })
        );
        assert!(pump.loops());
        assert_eq!(pump.base_note, 60);

        // Marker positions count decoded frames even for ima4.
        let fire = sound("Forest.sounds/FireCrackle.aiff");
        assert_eq!(
            fire.sustain_loop,
            Some(Loop {
                start: 0,
                end: 74234
            })
        );
        assert_eq!(fire.frames(), 1160 * 64);

        let checkpoint = sound("Main.sounds/Checkpoint.aiff");
        assert_eq!(checkpoint.base_note, 48);
        assert_eq!(checkpoint.sustain_loop, None);

        // Songs have an INST chunk without a sustain loop; WinSong's INST
        // even precedes its (empty) MARK chunk.
        for song in ["MenuSong.aiff", "WinSong.aiff"] {
            let s = sound(song);
            assert_eq!(s.sustain_loop, None);
            assert!(!s.loops());
        }
        assert_eq!(sound("AntHill.sounds/Explosion.aiff").base_note, 60);
    }

    #[test]
    fn ima4_packet_by_hand() {
        // Header 0x0080: predictor 0, step index 0 (step 7).
        let mut packet = vec![0x00, 0x00];
        // First byte: low nibble 0x7 then high nibble 0x9.
        packet.push(0x97);
        packet.resize(IMA4_PACKET_BYTES, 0);
        let out = decode_ima4(&packet, 1).unwrap();
        assert_eq!(out.len(), 64);
        // Nibble 7, step 7: diff = 0 + 7 + 3 + 1 = 11; index 0 + 8 = 8.
        assert_eq!(out[0], 11);
        // Nibble 9, step 16: diff = 2 + 4 = 6, subtracted; index 7.
        assert_eq!(out[1], 5);
        // Nibble 0, step 14: diff = 1; index 6.
        assert_eq!(out[2], 6);
    }

    /// The sample data of a file whose only chunk after `SSND`'s header is the
    /// data itself (true of every sound effect).
    fn raw_ssnd(bytes: &[u8]) -> &[u8] {
        let at = bytes.windows(4).position(|w| w == b"SSND").unwrap();
        &bytes[at + 16..]
    }

    #[test]
    fn real_ima4_follows_packet_headers() {
        // Each packet header resets or agrees with the running predictor, so
        // the first frame of each packet is within a couple of steps of it.
        let bytes = std::fs::read(original_data_dir().join("Audio/Main.sounds/Jump.aiff")).unwrap();
        let sound = parse(&bytes).unwrap();
        assert_eq!(sound.frames(), 243 * 64);
        for (i, packet) in raw_ssnd(&bytes).chunks_exact(IMA4_PACKET_BYTES).enumerate() {
            let header = i32::from(i16::from_be_bytes([packet[0], packet[1]]));
            let step = i32::from(IMA_STEP_TABLE[(header & 0x7F) as usize]);
            let first = i32::from(sound.samples[i * 64]);
            assert!((first - (header & !0x7F)).abs() <= 2 * step + 0x7F);
        }
        assert!(sound.samples.iter().any(|&s| s.unsigned_abs() > 4096));
    }

    #[test]
    fn twos_is_big_endian_pcm() {
        let bytes =
            std::fs::read(original_data_dir().join("Audio/Bonus.sounds/Bell.aiff")).unwrap();
        let expected: Vec<i16> = raw_ssnd(&bytes)
            .chunks_exact(2)
            .map(|b| i16::from_be_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(parse(&bytes).unwrap().samples, expected);
    }

    #[test]
    fn ima4_stereo_interleaves_packets() {
        let mut data = vec![0u8; 2 * IMA4_PACKET_BYTES];
        data[2] = 0x07; // left: first nibble 7, +11
        data[IMA4_PACKET_BYTES + 2] = 0x0F; // right: first nibble 15, -11
        let out = decode_ima4(&data, 2).unwrap();
        assert_eq!(out.len(), 128);
        assert_eq!(&out[..2], &[11, -11]);
    }

    #[test]
    fn ulaw_mirrors_negative_bytes() {
        let table = &ULAW_TO_PCM;
        assert_eq!(
            decode_xlaw(&[0x00, 0x7F, 0x80, 0xFF], table),
            [-32124, 0, 32124, 0]
        );
    }

    #[test]
    fn extended_floats() {
        let rate = |hex: u64, lo: u16| {
            let mut b = [0u8; 10];
            b[..8].copy_from_slice(&hex.to_be_bytes());
            b[8..].copy_from_slice(&lo.to_be_bytes());
            extended_to_f64(&b)
        };
        assert_eq!(rate(0x400E_AC44_0000_0000, 0), 44100.0);
        assert_eq!(rate(0x400D_AC44_0000_0000, 0), 22050.0);
        assert_eq!(rate(0x3FFF_8000_0000_0000, 0), 1.0);
        assert_eq!(rate(0xBFFF_8000_0000_0000, 0), -1.0);
        assert_eq!(rate(0, 0), 0.0);
        assert_eq!(rate(0x7FFF_8000_0000_0000, 0), f64::INFINITY);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(b"not an aiff").is_err());
        assert!(parse(b"FORM\0\0\0\x04WAVE").is_err());
        // A FORM with no chunks has no COMM.
        assert!(parse(b"FORM\0\0\0\x04AIFC").is_err());
        // A chunk that claims more data than there is.
        assert!(parse(b"FORM\0\0\0\x0cAIFCCOMM\xff\xff\xff\xff").is_err());
    }
}
