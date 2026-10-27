//! Audio on top of [kira](https://docs.rs/kira), exposed through Godot-style nodes.
//!
//! * [`AudioStream`]: sound data (WAV, Ogg Vorbis, MP3, FLAC), decoded up front or streamed
//!   while it plays.
//! * [`AudioPlayer`]: component of `AudioStreamPlayer` (music, UI) and `AudioStreamPlayer3D`
//!   (positional) nodes, with volume, pitch, looping and autoplay.
//! * [`AudioListener3D`]: where positional sound is heard from.
//! * [`Audio`] (ECS resource): the output, one-shot sounds ([`Audio::play`],
//!   [`Audio::play_at`]), master volume and finished notifications.
//!
//! [`audio_system`] syncs the scene with kira once per frame. Without a sound card (or with
//! [`AudioOutput::Silent`]) playback still advances with frame time, so game logic that
//! waits for sounds behaves the same in headless runs and tests.
//!
//! ```
//! use ab_audio::{audio_system, Audio, AudioNodes, AudioPlayer, AudioStream};
//! use ab_scene::SceneTree;
//!
//! let mut tree = SceneTree::new();
//! tree.world_mut().insert_resource(Audio::silent());
//! let beep = AudioStream::from_samples(8000, &[0.0; 800]); // 0.1 s
//! let player = tree.create_audio_stream_player("Beep", AudioPlayer::new(beep));
//! tree.add_child(tree.root(), player).unwrap();
//!
//! tree.component_mut::<AudioPlayer>(player).unwrap().play();
//! audio_system(tree.world_mut());
//! assert!(tree.component::<AudioPlayer>(player).unwrap().is_playing());
//! ```

mod audio;
mod chain;
pub mod clip;
mod components;
pub mod dsp;
pub mod effect;
mod event;
pub mod mixer;
mod nodes;
mod stream;

pub use audio::{audio_system, Audio, AudioOutput, SoundOptions, VoiceId, VoiceStats};
pub use clip::ClipEdit;
pub use components::{Attenuation, AudioListener3D, AudioPlayer, AudioZone, Rolloff, ZoneShape};
pub use dsp::meter::{Levels, MeterReading};
pub use effect::{EffectDesc, EffectKind};
pub use event::{ClipDesc, PickMode, SoundEvent, SoundEventDesc, StealMode, SOUND_EVENT_EXTENSION};
pub use mixer::{MixerSettings, MASTER};
pub use nodes::AudioNodes;
pub use stream::AudioStream;

/// Class names of the audio node types.
pub mod class {
    pub const AUDIO_STREAM_PLAYER: &str = "AudioStreamPlayer";
    pub const AUDIO_STREAM_PLAYER_3D: &str = "AudioStreamPlayer3D";
    pub const AUDIO_LISTENER_3D: &str = "AudioListener3D";
    pub const AUDIO_ZONE: &str = "AudioZone";
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("cannot decode audio: {0}")]
    Decode(String),
}
