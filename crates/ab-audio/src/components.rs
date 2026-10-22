use crate::stream::AudioStream;

/// Distance falloff of a positional sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attenuation {
    /// Up to this distance (m) from the listener the sound plays at full volume.
    pub min_distance: f32,
    /// From this distance on it is silent.
    pub max_distance: f32,
}

impl Default for Attenuation {
    fn default() -> Self {
        Self {
            min_distance: 1.0,
            max_distance: 40.0,
        }
    }
}

/// Plays an [`AudioStream`]: Godot's `AudioStreamPlayer` (no position: music, UI) or,
/// with `positional` set, `AudioStreamPlayer3D` (panned and attenuated from the node's
/// global position relative to the [`AudioListener3D`]).
///
/// Playback stops when the node leaves the tree; `autoplay` starts it whenever the node
/// enters it.
#[derive(Debug, Clone)]
pub struct AudioPlayer {
    pub stream: Option<AudioStream>,
    /// Gain in decibels (0 = unchanged, -6 ≈ half amplitude). Applied live.
    pub volume_db: f32,
    /// Playback speed and pitch together (2 = an octave up, twice as fast). Applied live.
    pub pitch_scale: f32,
    /// Restart from the beginning when the end is reached (music, ambience).
    pub looping: bool,
    /// Start playing as soon as the node is in the tree.
    pub autoplay: bool,
    /// `Some` for a 3D player.
    pub positional: Option<Attenuation>,
    pub(crate) command: Option<Command>,
    pub(crate) playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Play,
    Stop,
}

impl AudioPlayer {
    /// Signal emitted (on the player node) when its stream ends on its own.
    pub const FINISHED: &'static str = "finished";

    /// A non-positional player.
    pub fn new(stream: AudioStream) -> Self {
        Self {
            stream: Some(stream),
            volume_db: 0.0,
            pitch_scale: 1.0,
            looping: false,
            autoplay: false,
            positional: None,
            command: None,
            playing: false,
        }
    }

    /// A positional (3D) player with default attenuation.
    pub fn positional(stream: AudioStream) -> Self {
        Self {
            positional: Some(Attenuation::default()),
            ..Self::new(stream)
        }
    }

    pub fn with_volume_db(mut self, volume_db: f32) -> Self {
        self.volume_db = volume_db;
        self
    }

    pub fn with_pitch_scale(mut self, pitch_scale: f32) -> Self {
        self.pitch_scale = pitch_scale;
        self
    }

    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    pub fn with_autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }

    pub fn with_attenuation(mut self, attenuation: Attenuation) -> Self {
        self.positional = Some(attenuation);
        self
    }

    /// Plays the stream from the start (restarting it if already playing). Takes effect at
    /// the next audio update, at the end of the frame.
    pub fn play(&mut self) {
        self.command = Some(Command::Play);
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.command = Some(Command::Stop);
        self.playing = false;
    }

    /// Whether the stream is playing (or about to start).
    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

/// Where the player "hears" positional sounds from (Godot's `AudioListener3D`), usually a
/// child of the camera. The first listener in the tree with `current` set wins, else the
/// first listener; without any, the listener sits at the origin looking down -Z.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioListener3D {
    pub current: bool,
}

impl Default for AudioListener3D {
    fn default() -> Self {
        Self { current: true }
    }
}
