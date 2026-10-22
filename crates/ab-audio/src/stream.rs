use crate::AudioError;
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::Frame;
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

/// Decoded audio, ready to play (Godot's `AudioStream`). Cloning is cheap: the samples are
/// shared.
///
/// Sounds are decoded fully into memory, which suits effects and short-to-medium music
/// loops. Supported formats: WAV, Ogg Vorbis, MP3, FLAC.
#[derive(Clone)]
pub struct AudioStream {
    pub(crate) data: StaticSoundData,
}

impl AudioStream {
    /// Decodes an encoded file held in memory.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, AudioError> {
        let data = StaticSoundData::from_cursor(Cursor::new(bytes.into()))
            .map_err(|err| AudioError::Decode(err.to_string()))?;
        Ok(Self { data })
    }

    /// Wraps mono samples in `-1.0..=1.0` (procedural or synthesized sounds).
    pub fn from_samples(sample_rate: u32, samples: &[f32]) -> Self {
        let frames: Arc<[Frame]> = samples.iter().map(|&s| Frame::from_mono(s)).collect();
        Self {
            data: StaticSoundData {
                sample_rate: sample_rate.max(1),
                frames,
                settings: StaticSoundSettings::default(),
                slice: None,
            },
        }
    }

    pub fn duration(&self) -> Duration {
        self.data.duration()
    }

    pub fn sample_rate(&self) -> u32 {
        self.data.sample_rate
    }
}

impl fmt::Debug for AudioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioStream")
            .field("sample_rate", &self.data.sample_rate)
            .field("duration", &self.duration())
            .finish()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Minimal 16-bit mono PCM WAV file.
    pub(crate) fn wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn decodes_wav() {
        let stream = AudioStream::from_bytes(wav(8000, &[0; 4000])).unwrap();
        assert_eq!(stream.sample_rate(), 8000);
        assert_eq!(stream.duration(), Duration::from_millis(500));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            AudioStream::from_bytes(b"not audio".to_vec()),
            Err(AudioError::Decode(_))
        ));
    }

    #[test]
    fn from_samples() {
        let stream = AudioStream::from_samples(100, &[0.0; 50]);
        assert_eq!(stream.duration(), Duration::from_millis(500));
    }
}
