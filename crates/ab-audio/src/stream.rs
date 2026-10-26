use crate::AudioError;
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::sound::streaming::StreamingSoundData;
use kira::sound::FromFileError;
use kira::Frame;
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

/// Sound data ready to play (Godot's `AudioStream`). Cloning is cheap: the data is shared.
///
/// Two kinds, played the same way:
/// * **decoded** ([`AudioStream::from_bytes`]): every sample in memory, so playing starts at
///   once and costs nothing more; suits effects and short loops.
/// * **streaming** ([`AudioStream::streaming`]): only the encoded file is kept, decoded a
///   little ahead of playback on a thread of its own while it plays; suits music and long
///   ambience (a 4-minute Ogg song stays ~5 MB instead of ~85 MB of samples, and loading
///   it does not decode the whole file).
///
/// Supported formats: WAV, Ogg Vorbis, MP3, FLAC.
#[derive(Clone)]
pub struct AudioStream {
    pub(crate) data: StreamData,
}

#[derive(Clone)]
pub(crate) enum StreamData {
    Decoded(Box<StaticSoundData>),
    Streaming {
        /// The encoded file.
        bytes: Arc<[u8]>,
        sample_rate: u32,
        duration: Duration,
    },
}

impl AudioStream {
    /// Bytes it holds: the decoded samples, or the encoded file while streaming.
    pub fn memory_bytes(&self) -> usize {
        match &self.data {
            // Stereo f32 frames.
            StreamData::Decoded(data) => data.frames.len() * 8,
            StreamData::Streaming { bytes, .. } => bytes.len(),
        }
    }

    /// Decodes an encoded file held in memory.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, AudioError> {
        let data = StaticSoundData::from_cursor(Cursor::new(bytes.into()))
            .map_err(|err| AudioError::Decode(err.to_string()))?;
        Ok(Self {
            data: StreamData::Decoded(Box::new(data)),
        })
    }

    /// Keeps an encoded file to decode while it plays. Only the header is read now (to
    /// check the format and learn the duration).
    pub fn streaming(bytes: impl Into<Arc<[u8]>>) -> Result<Self, AudioError> {
        let bytes = bytes.into();
        let probe = streaming_data(&bytes)?;
        let duration = probe.duration();
        let sample_rate = if duration.is_zero() {
            0
        } else {
            (probe.num_frames() as f64 / duration.as_secs_f64()).round() as u32
        };
        Ok(Self {
            data: StreamData::Streaming {
                bytes,
                sample_rate,
                duration,
            },
        })
    }

    /// Wraps mono samples in `-1.0..=1.0` (procedural or synthesized sounds).
    pub fn from_samples(sample_rate: u32, samples: &[f32]) -> Self {
        let frames: Arc<[Frame]> = samples.iter().map(|&s| Frame::from_mono(s)).collect();
        Self {
            data: StreamData::Decoded(Box::new(StaticSoundData {
                sample_rate: sample_rate.max(1),
                frames,
                settings: StaticSoundSettings::default(),
                slice: None,
            })),
        }
    }

    /// Decoded while it plays ([`AudioStream::streaming`]).
    pub fn is_streaming(&self) -> bool {
        matches!(self.data, StreamData::Streaming { .. })
    }

    pub fn duration(&self) -> Duration {
        match &self.data {
            StreamData::Decoded(data) => data.duration(),
            StreamData::Streaming { duration, .. } => *duration,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match &self.data {
            StreamData::Decoded(data) => data.sample_rate,
            StreamData::Streaming { sample_rate, .. } => *sample_rate,
        }
    }
}

/// A fresh decoder over a streaming file (kira consumes one per playback).
pub(crate) fn streaming_data(
    bytes: &Arc<[u8]>,
) -> Result<StreamingSoundData<FromFileError>, AudioError> {
    StreamingSoundData::from_cursor(Cursor::new(bytes.clone()))
        .map_err(|err| AudioError::Decode(err.to_string()))
}

impl fmt::Debug for AudioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioStream")
            .field("streaming", &self.is_streaming())
            .field("sample_rate", &self.sample_rate())
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
    fn streaming_reads_only_the_header() {
        let stream = AudioStream::streaming(wav(8000, &[0; 4000])).unwrap();
        assert!(stream.is_streaming());
        assert_eq!(stream.sample_rate(), 8000);
        assert_eq!(stream.duration(), Duration::from_millis(500));
        assert!(matches!(
            AudioStream::streaming(b"not audio".to_vec()),
            Err(AudioError::Decode(_))
        ));
    }

    #[test]
    fn from_samples() {
        let stream = AudioStream::from_samples(100, &[0.0; 50]);
        assert_eq!(stream.duration(), Duration::from_millis(500));
    }
}
