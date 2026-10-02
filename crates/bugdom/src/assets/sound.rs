//! AIFF sounds and music (`Audio/*.aiff`, `Audio/*.sounds/*.aiff`), decoded by
//! `bugdom_formats::aiff` and played through Bevy's audio with a custom
//! [`Decodable`] source, so no extra audio codec is needed.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::audio::{AddAudioSource, Decodable, Source};
use bevy::prelude::*;
use bugdom_formats::aiff;

pub(super) fn plugin(app: &mut App) {
    app.add_audio_source::<Sound>()
        .init_asset_loader::<AiffLoader>();
}

/// A decoded sound: play it with `AudioPlayer::<Sound>`.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct Sound {
    /// Interleaved samples in -1..1.
    pub samples: Arc<[f32]>,
    pub channels: NonZero<u16>,
    pub sample_rate: NonZero<u32>,
    /// The MIDI note the sound plays at unshifted. Sound effects are pitched
    /// relative to it (`PlayEffect_Parms` in original/src/System/Sound.c).
    pub base_note: i8,
    /// Where playback restarts when the sound is looped, in frames.
    pub loop_start: Option<u32>,
}

/// Loads `.aiff` files as [`Sound`]s.
#[derive(Default, TypePath)]
pub struct AiffLoader;

impl AssetLoader for AiffLoader {
    type Asset = Sound;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Sound, BevyError> {
        let bytes = super::read_all(reader).await?;
        let sound = aiff::parse(&bytes)?;
        let channels = NonZero::new(sound.channels).ok_or("sound has no channels")?;
        // Rates are whole numbers apart from rounding in the 80-bit float.
        let sample_rate =
            NonZero::new(sound.sample_rate.round() as u32).ok_or("sound has a zero sample rate")?;
        let loop_start = sound
            .loops()
            .then(|| sound.sustain_loop.map(|l| l.start))
            .flatten();
        Ok(Sound {
            samples: sound
                .samples
                .iter()
                .map(|&s| f32::from(s) / 32768.0)
                .collect(),
            channels,
            sample_rate,
            base_note: sound.base_note,
            loop_start,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["aiff"]
    }
}

impl Decodable for Sound {
    type Decoder = SoundDecoder;

    fn decoder(&self) -> SoundDecoder {
        SoundDecoder {
            sound: self.clone(),
            position: 0,
        }
    }
}

/// Plays a [`Sound`] once from the start. Looping uses Bevy's playback
/// settings, which restart from the beginning; sustain loops that start
/// later are for the audio work in Phase 4.
pub struct SoundDecoder {
    sound: Sound,
    position: usize,
}

impl Iterator for SoundDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.sound.samples.get(self.position).copied();
        self.position += 1;
        sample
    }
}

impl Source for SoundDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.sound.samples.len().saturating_sub(self.position))
    }

    fn channels(&self) -> NonZero<u16> {
        self.sound.channels
    }

    fn sample_rate(&self) -> NonZero<u32> {
        self.sound.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        let frames = self.sound.samples.len() / usize::from(self.sound.channels.get());
        Some(Duration::from_secs_f64(
            frames as f64 / f64::from(self.sound.sample_rate.get()),
        ))
    }
}
