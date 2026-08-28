//! Sound effects: the presentation half of `GameEvent`.
//!
//! The backend announces events in snapshots; turning them into audible
//! beeps is the frontend's business (ARCHITECTURE.md: the backend stays
//! I/O-free). Terminal bells cannot express pitch, so tones are
//! synthesized as sine waves and played through `rodio`.
//!
//! Two pitches, per the game design: paddle hits and point scores share
//! 440 Hz; the match end gets 880 Hz.

use std::f32::consts::TAU;

use pong_core::GameEvent;
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamHandle};

/// Synthesis sample rate, Hz.
pub const SAMPLE_RATE: u32 = 44_100;

/// Tone duration, milliseconds.
const TONE_MS: u64 = 90;

/// Linear attack/release ramp, milliseconds: keeps the onset and offset
/// click-free.
const ENVELOPE_MS: u64 = 5;

/// Peak amplitude of a tone.
const VOLUME: f32 = 0.25;

/// Frequency (Hz) of the shared hit/score beep.
const HIT_FREQ: f32 = 440.0;

/// Frequency (Hz) of the game-over beep: one octave higher.
const GAME_OVER_FREQ: f32 = 880.0;

/// Synthesizes one enveloped sine tone.
fn tone(freq: f32) -> Vec<f32> {
    let total = TONE_MS * SAMPLE_RATE as u64 / 1000;
    let ramp = ENVELOPE_MS * SAMPLE_RATE as u64 / 1000;
    (0..total)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let envelope = if i < ramp {
                i as f32 / ramp as f32
            } else if i >= total - ramp {
                (total - i) as f32 / ramp as f32
            } else {
                1.0
            };
            VOLUME * envelope * (TAU * freq * t).sin()
        })
        .collect()
}

/// Plays the two game beeps. `None` from [`SoundPlayer::new`] means no
/// audio device is available and the caller should fall back to the
/// terminal bell.
pub struct SoundPlayer {
    // The output stream must outlive every `play_raw`; rodio stops
    // playing the moment it is dropped.
    _stream: OutputStream,
    handle: OutputStreamHandle,
    hit: Vec<f32>,
    game_over: Vec<f32>,
}

impl SoundPlayer {
    /// Opens the default audio output; `None` when there is none.
    pub fn new() -> Option<Self> {
        let (stream, handle) = OutputStream::try_default().ok()?;
        Some(Self {
            _stream: stream,
            handle,
            hit: tone(HIT_FREQ),
            game_over: tone(GAME_OVER_FREQ),
        })
    }

    /// Plays the beep for one game event.
    pub fn play(&self, event: GameEvent) {
        let samples = match event {
            GameEvent::PaddleHit | GameEvent::PointScored => &self.hit,
            GameEvent::GameOver => &self.game_over,
        };
        // A dead stream just means silence; not worth reporting.
        let _ = self
            .handle
            .play_raw(SamplesBuffer::new(1, SAMPLE_RATE, samples.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_length_matches_the_duration() {
        let expected = TONE_MS * SAMPLE_RATE as u64 / 1000;
        assert_eq!(tone(HIT_FREQ).len() as u64, expected);
        assert_eq!(tone(GAME_OVER_FREQ).len() as u64, expected);
    }

    #[test]
    fn tone_starts_and_ends_silently() {
        // The envelope ramp removes the click at both ends. The discrete
        // ramp leaves at most one step of amplitude (<0.1% of full scale)
        // on the final sample.
        let step = VOLUME / (ENVELOPE_MS as f32 * SAMPLE_RATE as f32 / 1000.0);
        for freq in [HIT_FREQ, GAME_OVER_FREQ] {
            let samples = tone(freq);
            assert!(samples.first().is_some_and(|s| s.abs() <= step));
            assert!(samples.last().is_some_and(|s| s.abs() <= step));
        }
    }

    #[test]
    fn tone_stays_within_the_volume_ceiling() {
        for freq in [HIT_FREQ, GAME_OVER_FREQ] {
            assert!(tone(freq).iter().all(|s| s.abs() <= VOLUME + 1e-6));
        }
    }
}
