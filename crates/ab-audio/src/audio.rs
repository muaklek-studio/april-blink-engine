use crate::chain::{Chain, VoiceEffectBuilder};
use crate::components::{Attenuation, AudioListener3D, AudioPlayer, AudioZone, Command};
use crate::dsp::meter::{AtomicF32, Levels};
use crate::dsp::{gain_to_db, SILENCE_DB};
use crate::event::{SoundEvent, StealMode};
use crate::mixer::{Mixer, MixerSettings};
use crate::stream::{streaming_data, AudioStream, StreamData};
use ab_core::{Affine3A, Quat, Time, Vec3};
use ab_ecs::{Entity, World};
use ab_scene::{NodeHandle, NodeInfo, Signal, SignalQueue, Transform};
use kira::backend::mock::{MockBackend, MockBackendSettings};
use kira::backend::Backend;
use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::sound::{EndPosition, FromFileError, PlaybackPosition, PlaybackState, Region};
use kira::track::{
    MainTrackBuilder, SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle,
};
use kira::{AudioManager, AudioManagerSettings, Capacities, Decibels, Tween};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Where sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioOutput {
    /// The default sound card; falls back to [`AudioOutput::Silent`] if it cannot be opened
    /// or the `device` feature is off.
    #[default]
    Device,
    /// No output, but playback still advances with frame time, so sounds finish and
    /// `is_playing` behaves exactly as with a device (headless runs, servers, tests).
    Silent,
}

/// Volume and pitch for a one-shot sound.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundOptions {
    pub volume_db: f32,
    pub pitch_scale: f32,
    /// Falloff for [`Audio::play_at`].
    pub attenuation: Attenuation,
}

impl Default for SoundOptions {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pitch_scale: 1.0,
            attenuation: Attenuation::default(),
        }
    }
}

/// A sound started by [`Audio::play_event`] (or a player), to stop or ask about later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(u64);

/// How many sounds the mixer is handling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceStats {
    /// Playing and heard.
    pub real: usize,
    /// Looping sounds out of earshot, paused until the listener comes back.
    pub virtual_voices: usize,
    /// Plays refused this session: over a voice limit, in cooldown or out of range.
    pub refused: u64,
    /// Voices stopped to make room for more important ones.
    pub stolen: u64,
}

/// Sample rate of the silent backend: low, since nothing is heard.
const SILENT_SAMPLE_RATE: u32 = 8000;
/// Frames the mixer renders per process call (kira's default internal buffer size).
const MIX_CHUNK: usize = 128;
/// Lowest playback rate: zero would freeze a sound forever, negative would play it backwards.
const MIN_PITCH_SCALE: f32 = 0.01;
/// How long a stolen or stopped voice takes to fade out.
const STOP_FADE: Duration = Duration::from_millis(30);
/// Speed of sound for doppler, m/s.
const SPEED_OF_SOUND: f32 = 343.0;
/// A fully occluded sound: this much quieter and filtered from this frequency.
const OCCLUDED_DB: f32 = -9.0;
const OCCLUDED_CUTOFF_HZ: f32 = 900.0;
/// Seconds for occlusion to follow a change (walls do not pop in).
const OCCLUSION_TIME: f32 = 0.15;

enum Output {
    #[cfg(feature = "device")]
    Device(Box<AudioManager<kira::DefaultBackend>>),
    Silent {
        manager: Box<AudioManager<MockBackend>>,
        /// Frames of elapsed time not yet mixed.
        pending_frames: f64,
    },
}

/// Runs `$body` with `$m` bound to the active `AudioManager`, whatever its backend.
macro_rules! with_manager {
    ($output:expr, $m:ident => $body:expr) => {
        match $output {
            #[cfg(feature = "device")]
            Output::Device($m) => $body,
            Output::Silent { manager: $m, .. } => $body,
        }
    };
}

/// Plays configured sound data on `$target` (an `AudioManager` or a track), giving back a
/// [`Sound`] or the error as text.
macro_rules! play_on {
    ($target:expr, $playback:expr) => {
        match $playback {
            Playback::Decoded(data) => $target
                .play(data)
                .map(Sound::Decoded)
                .map_err(|err| err.to_string()),
            Playback::Streaming(data) => $target
                .play(data)
                .map(Sound::Streaming)
                .map_err(|err| err.to_string()),
        }
    };
}

/// Sound data set up for one playback.
enum Playback {
    Decoded(StaticSoundData),
    Streaming(StreamingSoundData<FromFileError>),
}

/// A sound kira is playing.
enum Sound {
    Decoded(StaticSoundHandle),
    Streaming(StreamingSoundHandle<FromFileError>),
}

impl Sound {
    fn stop(&mut self, fade: Duration) {
        let tween = Tween {
            duration: fade,
            ..Tween::default()
        };
        match self {
            Sound::Decoded(sound) => sound.stop(tween),
            Sound::Streaming(sound) => sound.stop(tween),
        }
    }

    fn pause(&mut self) {
        match self {
            Sound::Decoded(sound) => sound.pause(Tween::default()),
            Sound::Streaming(sound) => sound.pause(Tween::default()),
        }
    }

    fn resume(&mut self) {
        match self {
            Sound::Decoded(sound) => sound.resume(Tween::default()),
            Sound::Streaming(sound) => sound.resume(Tween::default()),
        }
    }

    fn set_volume(&mut self, volume_db: f32) {
        let volume = decibels(volume_db);
        match self {
            Sound::Decoded(sound) => sound.set_volume(volume, Tween::default()),
            Sound::Streaming(sound) => sound.set_volume(volume, Tween::default()),
        }
    }

    fn set_playback_rate(&mut self, rate: f64) {
        match self {
            Sound::Decoded(sound) => sound.set_playback_rate(rate, Tween::default()),
            Sound::Streaming(sound) => sound.set_playback_rate(rate, Tween::default()),
        }
    }

    fn state(&self) -> PlaybackState {
        match self {
            Sound::Decoded(sound) => sound.state(),
            Sound::Streaming(sound) => sound.state(),
        }
    }

    fn position(&self) -> f64 {
        match self {
            Sound::Decoded(sound) => sound.position(),
            Sound::Streaming(sound) => sound.position(),
        }
    }

    /// Logs decoding errors of a streaming sound (a damaged file stops it early).
    fn report_errors(&mut self) {
        if let Sound::Streaming(sound) = self {
            while let Some(err) = sound.pop_error() {
                warn!("audio stream failed: {err}");
            }
        }
    }
}

fn decibels(db: f32) -> Decibels {
    if db <= SILENCE_DB || !db.is_finite() {
        Decibels::SILENCE
    } else {
        Decibels(db)
    }
}

/// One sound being played.
struct Voice {
    id: u64,
    sound: Sound,
    /// The 3D track of a positional voice (dropping it ends the sound).
    track: Option<SpatialTrackHandle>,
    /// Occlusion low-pass cutoff on the track.
    cutoff: Option<Arc<AtomicF32>>,
    owner: Option<Entity>,
    event: Option<u64>,
    priority: u8,
    started: f64,
    /// Mixer layout it was started on.
    generation: u64,
    attenuation: Option<Attenuation>,
    position: Vec3,
    velocity: Vec3,
    /// Linear distance gain applied to the track.
    distance_gain: f32,
    occlusion: f32,
    occlusion_target: f32,
    /// Volume and rate it plays at (event randomization and player settings included).
    volume_db: f32,
    rate: f64,
    /// The owning player's own volume and rate within those (they change live).
    player_volume_db: f32,
    player_rate: f64,
    applied_volume_db: f32,
    applied_rate: f64,
    applied_track_db: f32,
    /// Seconds into the sound to fade out at a streamed clip's end, and the fade.
    stream_end: Option<(f64, f32)>,
    looping: bool,
    paused_out_of_range: bool,
    /// Stopped on purpose (not finished on its own).
    stopped: bool,
}

impl Voice {
    fn stop(&mut self, fade: Duration) {
        if !self.stopped {
            self.stopped = true;
            self.sound.stop(fade);
        }
    }

    /// How loud it is where the listener is (for stealing the quietest).
    fn audibility(&self) -> f32 {
        crate::dsp::db_to_gain(self.volume_db)
            * self.distance_gain
            * crate::dsp::db_to_gain(OCCLUDED_DB * self.occlusion)
    }
}

/// What a player last started, to notice changes needing a restart.
#[derive(Default)]
struct PlayerVoice {
    voice: Option<u64>,
    autoplayed: bool,
    positional: Option<Attenuation>,
    bus: String,
}

/// A sound to start.
struct Start<'a> {
    stream: &'a AudioStream,
    stream_end: Option<(f64, f32)>,
    volume_db: f32,
    rate: f64,
    looping: bool,
    bus: &'a str,
    attenuation: Option<Attenuation>,
    position: Vec3,
    owner: Option<Entity>,
    event: Option<(u64, u32, StealMode)>,
    priority: u8,
}

/// ECS resource owning the audio output (kira), the mixer, the listener and every playing
/// sound.
///
/// [`audio_system`] syncs it with the scene once per frame: it moves the listener to the
/// current [`AudioListener3D`], blends zone snapshots, starts, stops and updates
/// [`AudioPlayer`] nodes and follows 3D sounds around. Fire-and-forget sounds go through
/// [`Audio::play_event`], [`Audio::play`] and [`Audio::play_at`].
pub struct Audio {
    output: Output,
    listener: ListenerHandle,
    listener_pose: (Vec3, Quat),
    listener_velocity: Vec3,
    mixer: Mixer,
    voices: Vec<Voice>,
    next_voice: u64,
    players: HashMap<Entity, PlayerVoice>,
    finished: Vec<NodeHandle>,
    master_volume_db: f32,
    /// Seconds of audio time.
    clock: f64,
    rng: u32,
    /// Event → when it last started.
    cooldowns: HashMap<u64, f64>,
    stats: VoiceStats,
}

impl Audio {
    pub fn new(output: AudioOutput) -> Self {
        let master_track = || {
            let (chain, builder) = Chain::new();
            (chain, MainTrackBuilder::new().with_effect(builder))
        };
        let (master, mut output) = match output {
            AudioOutput::Device => {
                let (chain, main) = master_track();
                match open_device(main) {
                    Some(output) => (chain, output),
                    // The failed attempt took its chain along: a new one for the fallback.
                    None => {
                        let (chain, main) = master_track();
                        (chain, silent_output(main))
                    }
                }
            }
            AudioOutput::Silent => {
                let (chain, main) = master_track();
                (chain, silent_output(main))
            }
        };
        let listener = with_manager!(&mut output, m => m.add_listener(mint_vec(Vec3::ZERO), mint_quat(Quat::IDENTITY)))
            .expect("a fresh audio manager has room for a listener");
        let mut audio = Self {
            output,
            listener,
            listener_pose: (Vec3::ZERO, Quat::IDENTITY),
            listener_velocity: Vec3::ZERO,
            mixer: Mixer::new(master),
            voices: Vec::new(),
            next_voice: 1,
            players: HashMap::new(),
            finished: Vec::new(),
            master_volume_db: 0.0,
            clock: 0.0,
            rng: 0x9e37_79b9,
            cooldowns: HashMap::new(),
            stats: VoiceStats::default(),
        };
        audio.configure_mixer(MixerSettings::default());
        audio
    }

    /// An audio resource without output; see [`AudioOutput::Silent`].
    pub fn silent() -> Self {
        Self::new(AudioOutput::Silent)
    }

    /// Whether sound is discarded (no device, or [`AudioOutput::Silent`]).
    pub fn is_silent(&self) -> bool {
        matches!(self.output, Output::Silent { .. })
    }

    pub fn master_volume_db(&self) -> f32 {
        self.master_volume_db
    }

    /// Gain applied to everything, in decibels (on top of the master bus).
    pub fn set_master_volume_db(&mut self, volume_db: f32) {
        self.master_volume_db = volume_db;
        with_manager!(&mut self.output, m => m.main_track().set_volume(decibels(volume_db), Tween::default()));
    }

    /// Takes the project's mixer (`audio.toml`). Buses whose layout changed are rebuilt,
    /// which stops what plays on them (players restart).
    pub fn configure_mixer(&mut self, settings: MixerSettings) {
        let generation = self.mixer.generation;
        with_manager!(&mut self.output, m => self.mixer.configure(m, settings));
        if self.mixer.generation != generation {
            for voice in &mut self.voices {
                voice.stopped = true;
            }
        }
    }

    pub fn mixer_settings(&self) -> &MixerSettings {
        self.mixer.settings()
    }

    /// Starts or stops a mixer snapshot (it fades over its own times). `false` if the mixer
    /// has no snapshot of that name.
    pub fn set_snapshot(&mut self, name: &str, active: bool) -> bool {
        self.mixer.set_snapshot(name, active)
    }

    /// How far a snapshot has taken over, 0..=1.
    pub fn snapshot_level(&self, name: &str) -> f32 {
        self.mixer.snapshot_level(name)
    }

    /// Fades a bus by `offset_db` on top of its mixer settings over `seconds` (a settings
    /// menu's volume sliders; cutscenes turning the music down).
    pub fn set_bus_volume(&mut self, bus: &str, offset_db: f32, seconds: f32) {
        self.mixer.set_volume_offset(bus, offset_db, seconds);
    }

    /// Live levels of a bus (meters, the spectrum), by name (`"Master"` included).
    pub fn bus_levels(&self, bus: &str) -> Option<Arc<Levels>> {
        self.mixer.levels(bus).cloned()
    }

    pub fn voice_stats(&self) -> VoiceStats {
        let mut stats = self.stats;
        stats.real = self
            .voices
            .iter()
            .filter(|v| !v.paused_out_of_range && !v.stopped)
            .count();
        stats.virtual_voices = self.voices.iter().filter(|v| v.paused_out_of_range).count();
        stats
    }

    /// Global position and rotation of the listener as of the last update.
    pub fn listener_transform(&self) -> (Vec3, Quat) {
        self.listener_pose
    }

    /// Players whose stream ended on its own during the last update (Godot's `finished`
    /// signal). Read it from `process`.
    pub fn finished(&self) -> &[NodeHandle] {
        &self.finished
    }

    /// Plays a non-positional one-shot (UI clicks, stingers).
    pub fn play(&mut self, stream: &AudioStream, options: SoundOptions) {
        self.start(Start {
            stream,
            stream_end: None,
            volume_db: options.volume_db,
            rate: f64::from(options.pitch_scale.max(MIN_PITCH_SCALE)),
            looping: false,
            bus: "",
            attenuation: None,
            position: Vec3::ZERO,
            owner: None,
            event: None,
            priority: 128,
        });
    }

    /// Plays a one-shot at a global position (impacts, footsteps, explosions).
    pub fn play_at(&mut self, stream: &AudioStream, position: Vec3, options: SoundOptions) {
        self.start(Start {
            stream,
            stream_end: None,
            volume_db: options.volume_db,
            rate: f64::from(options.pitch_scale.max(MIN_PITCH_SCALE)),
            looping: false,
            bus: "",
            attenuation: Some(options.attenuation),
            position,
            owner: None,
            event: None,
            priority: 128,
        });
    }

    /// Plays a sound event, 2D. `None` when it does not play (no clips, cooldown, voice
    /// limits).
    pub fn play_event(&mut self, event: &SoundEvent) -> Option<VoiceId> {
        self.start_event(event, None, None, 0.0, 1.0, None)
    }

    /// Plays a sound event at a global position (3D with the event's spatial settings, or
    /// default attenuation if it has none).
    pub fn play_event_at(&mut self, event: &SoundEvent, position: Vec3) -> Option<VoiceId> {
        let attenuation = event.desc().spatial.clone().unwrap_or_default();
        self.start_event(event, Some((attenuation, position)), None, 0.0, 1.0, None)
    }

    /// Plays a sound event with extra volume and pitch on top of its own randomization, 2D
    /// or (with `position`) 3D.
    pub fn play_event_with(
        &mut self,
        event: &SoundEvent,
        volume_db: f32,
        pitch_scale: f32,
        position: Option<Vec3>,
    ) -> Option<VoiceId> {
        let spatial = position.map(|p| (event.desc().spatial.clone().unwrap_or_default(), p));
        self.start_event(event, spatial, None, volume_db, pitch_scale, None)
    }

    /// Stops a voice, fading out over `fade_seconds`.
    pub fn stop_voice(&mut self, id: VoiceId, fade_seconds: f32) {
        if let Some(voice) = self.voices.iter_mut().find(|v| v.id == id.0) {
            voice.stop(Duration::from_secs_f32(fade_seconds.max(0.0)));
        }
    }

    pub fn is_voice_playing(&self, id: VoiceId) -> bool {
        self.voices.iter().any(|v| v.id == id.0 && !v.stopped)
    }

    /// Stops every sound and clears effect tails (players report not playing; they
    /// restart when played again).
    pub fn stop_all(&mut self) {
        for voice in &mut self.voices {
            voice.stop(STOP_FADE);
        }
        self.mixer.flush();
    }

    /// 3D voices that want occlusion, with where they are: the engine casts a ray from the
    /// listener to each and reports the result with [`Audio::set_occlusion`].
    pub fn occlusion_requests(&self) -> Vec<(VoiceId, Vec3)> {
        self.voices
            .iter()
            .filter(|v| !v.stopped && v.attenuation.as_ref().is_some_and(|a| a.occlusion))
            .map(|v| (VoiceId(v.id), v.position))
            .collect()
    }

    /// How blocked a voice is from the listener: 0 clear .. 1 behind a wall.
    pub fn set_occlusion(&mut self, id: VoiceId, amount: f32) {
        if let Some(voice) = self.voices.iter_mut().find(|v| v.id == id.0) {
            voice.occlusion_target = if amount.is_finite() {
                amount.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Picks a clip of `event`, randomizes it and starts it.
    fn start_event(
        &mut self,
        event: &SoundEvent,
        spatial: Option<(Attenuation, Vec3)>,
        owner: Option<Entity>,
        volume_db: f32,
        pitch_scale: f32,
        overrides: Option<(&str, bool)>,
    ) -> Option<VoiceId> {
        let desc = event.desc();
        if desc.cooldown > 0.0 {
            if let Some(&last) = self.cooldowns.get(&event.id()) {
                if self.clock - last < f64::from(desc.cooldown) {
                    self.stats.refused += 1;
                    return None;
                }
            }
        }
        let mut random = || self.random();
        let clip = event.pick(&mut random)?.clone();
        let spread = |range: f32, r: f32| {
            if range.is_finite() {
                (r * 2.0 - 1.0) * range.abs()
            } else {
                0.0
            }
        };
        let (r1, r2) = (self.random(), self.random());
        let volume = desc.volume_db + spread(desc.volume_random_db, r1) + volume_db;
        let semitones = desc.pitch_semitones + spread(desc.pitch_random_semitones, r2);
        let rate =
            2f64.powf(f64::from(semitones) / 12.0) * f64::from(pitch_scale.max(MIN_PITCH_SCALE));
        let (bus, looping) = overrides.unwrap_or((&desc.bus, desc.looping));
        let bus = if bus.is_empty() {
            desc.bus.as_str()
        } else {
            bus
        };
        let (attenuation, position) = match spatial {
            Some((a, p)) => (Some(a), p),
            None => (None, Vec3::ZERO),
        };
        let id = self.start(Start {
            stream: &clip.stream,
            stream_end: clip.stream_end,
            volume_db: volume,
            rate,
            looping,
            bus,
            attenuation,
            position,
            owner,
            event: Some((event.id(), desc.max_voices, desc.steal)),
            priority: desc.priority,
        })?;
        self.cooldowns.insert(event.id(), self.clock);
        Some(id)
    }

    /// Makes room under the voice limits and starts the sound.
    fn start(&mut self, start: Start<'_>) -> Option<VoiceId> {
        let distance_gain = start.attenuation.as_ref().map_or(1.0, |a| {
            a.gain((start.position - self.listener_pose.0).length())
        });
        if distance_gain <= 0.0 && !start.looping {
            // Too far to hear: not worth a voice.
            self.stats.refused += 1;
            return None;
        }
        let audibility = crate::dsp::db_to_gain(start.volume_db) * distance_gain;

        if let Some((event, max, steal)) = start.event {
            let max = max as usize;
            if max > 0 {
                let mine: Vec<usize> = (0..self.voices.len())
                    .filter(|&i| {
                        let v = &self.voices[i];
                        v.event == Some(event) && !v.stopped && v.owner.is_none()
                    })
                    .collect();
                if mine.len() >= max {
                    let victim = match steal {
                        StealMode::None => None,
                        StealMode::Oldest => mine.iter().copied().min_by(|&a, &b| {
                            self.voices[a].started.total_cmp(&self.voices[b].started)
                        }),
                        StealMode::Quietest => mine.iter().copied().min_by(|&a, &b| {
                            self.voices[a]
                                .audibility()
                                .total_cmp(&self.voices[b].audibility())
                        }),
                    };
                    match victim {
                        Some(i) => {
                            self.voices[i].stop(STOP_FADE);
                            self.stats.stolen += 1;
                        }
                        None => {
                            self.stats.refused += 1;
                            return None;
                        }
                    }
                }
            }
        }

        let limit = self.mixer.settings().max_voices.max(1) as usize;
        let live = || {
            self.voices
                .iter()
                .filter(|v| !v.stopped && !v.paused_out_of_range)
                .count()
        };
        if live() >= limit {
            // The least important, quietest voice goes if it matters less than the new one.
            let worst = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| !v.stopped && !v.paused_out_of_range)
                .max_by(|(_, a), (_, b)| {
                    a.priority
                        .cmp(&b.priority)
                        .then(b.audibility().total_cmp(&a.audibility()))
                })
                .map(|(i, _)| i);
            match worst {
                Some(i)
                    if self.voices[i].priority > start.priority
                        || (self.voices[i].priority == start.priority
                            && self.voices[i].audibility() < audibility) =>
                {
                    self.voices[i].stop(STOP_FADE);
                    self.stats.stolen += 1;
                }
                _ => {
                    self.stats.refused += 1;
                    return None;
                }
            }
        }

        let data = configure(start.stream, start.volume_db, start.rate, start.looping)?;
        let Self {
            output,
            mixer,
            listener,
            ..
        } = self;
        let bus_track = mixer.track_mut(Some(start.bus));
        let (track, cutoff) = match &start.attenuation {
            Some(attenuation) => {
                let cutoff = attenuation
                    .occlusion
                    .then(|| Arc::new(AtomicF32::new(crate::dsp::biquad::VoiceFilter::OPEN_HZ)));
                let mut builder = spatial_builder(attenuation);
                if let Some(cutoff) = &cutoff {
                    builder = builder.with_effect(VoiceEffectBuilder {
                        cutoff: cutoff.clone(),
                    });
                }
                builder = builder.volume(decibels(gain_to_db(distance_gain)));
                let position = mint_vec(start.position);
                let created = match bus_track {
                    Some(bus) => bus.add_spatial_sub_track(listener.id(), position, builder),
                    None => {
                        with_manager!(output, m => m.add_spatial_sub_track(listener.id(), position, builder))
                    }
                };
                match created {
                    Ok(track) => (Some(track), cutoff),
                    Err(err) => {
                        warn!("cannot create a 3D audio track: {err}");
                        return None;
                    }
                }
            }
            None => (None, None),
        };
        let played = match (track, mixer.track_mut(Some(start.bus))) {
            (Some(mut track), _) => play_on!(track, data).map(|s| (s, Some(track))),
            (None, Some(bus)) => play_on!(bus, data).map(|s| (s, None)),
            (None, None) => with_manager!(output, m => play_on!(m, data)).map(|s| (s, None)),
        };
        let (mut sound, track) = match played {
            Ok(played) => played,
            Err(err) => {
                warn!("cannot play sound: {err}");
                return None;
            }
        };
        let paused = distance_gain <= 0.0;
        if paused {
            sound.pause();
        }
        let id = self.next_voice;
        self.next_voice += 1;
        self.voices.push(Voice {
            id,
            sound,
            track,
            cutoff,
            owner: start.owner,
            event: start.event.map(|(e, _, _)| e),
            priority: start.priority,
            started: self.clock,
            generation: self.mixer.generation,
            attenuation: start.attenuation,
            position: start.position,
            velocity: Vec3::ZERO,
            distance_gain,
            occlusion: 0.0,
            occlusion_target: 0.0,
            volume_db: start.volume_db,
            rate: start.rate,
            player_volume_db: 0.0,
            player_rate: 1.0,
            applied_volume_db: start.volume_db,
            applied_rate: start.rate,
            applied_track_db: gain_to_db(distance_gain),
            stream_end: start.stream_end,
            looping: start.looping,
            paused_out_of_range: paused,
            stopped: false,
        });
        Some(VoiceId(id))
    }

    /// Syncs listener, zones and players with the scene, updates every voice, then advances
    /// the silent backend by `delta` seconds. Normally run by [`audio_system`].
    pub fn update(&mut self, world: &World, delta: f32) {
        self.finished.clear();
        self.clock += f64::from(delta.max(0.0));
        self.update_listener(world, delta);
        self.update_zones(world);
        self.mixer.update(delta);
        self.update_players(world, delta);
        self.update_voices(delta);
        self.advance(delta);
        self.poll(Some(world));
        if let Some(mut queue) = world.resource_mut::<SignalQueue>() {
            for &node in &self.finished {
                queue.push(Signal::new(node, AudioPlayer::FINISHED));
            }
        }
    }

    /// Advances what plays without a scene (an editor's previews): the mixer, the voices
    /// and the silent backend, leaving players and the listener alone.
    pub fn update_preview(&mut self, delta: f32) {
        self.finished.clear();
        self.clock += f64::from(delta.max(0.0));
        self.mixer.update(delta);
        self.update_voices(delta);
        self.advance(delta);
        self.poll(None);
    }

    fn update_listener(&mut self, world: &World, delta: f32) {
        let mut chosen: Option<(bool, Affine3A)> = None;
        for (listener, transform, info) in world
            .query::<(&AudioListener3D, &Transform, &NodeInfo)>()
            .iter()
        {
            if info.in_tree && chosen.is_none_or(|(current, _)| !current && listener.current) {
                chosen = Some((listener.current, transform.global));
            }
        }
        let pose = chosen.map_or((Vec3::ZERO, Quat::IDENTITY), |(_, global)| {
            let (_, rotation, translation) = global.to_scale_rotation_translation();
            // A listener under a node scaled to zero must not make every sound's panning NaN.
            let rotation = if rotation.is_finite() && rotation.length() > 1e-6 {
                rotation.normalize()
            } else {
                Quat::IDENTITY
            };
            let translation = if translation.is_finite() {
                translation
            } else {
                Vec3::ZERO
            };
            (translation, rotation)
        });
        self.listener_velocity = velocity(self.listener_pose.0, pose.0, delta);
        if pose != self.listener_pose {
            self.listener_pose = pose;
            self.listener
                .set_position(mint_vec(pose.0), Tween::default());
            self.listener
                .set_orientation(mint_quat(pose.1), Tween::default());
        }
    }

    fn update_zones(&mut self, world: &World) {
        let listener = self.listener_pose.0;
        for (zone, transform, info) in world.query::<(&AudioZone, &Transform, &NodeInfo)>().iter() {
            if !info.in_tree || zone.snapshot.is_empty() {
                continue;
            }
            let local = transform.global.inverse().transform_point3(listener);
            let amount = zone.amount(local);
            if amount > 0.0 {
                self.mixer.set_zone_amount(&zone.snapshot, amount);
            }
        }
    }

    fn update_players(&mut self, world: &World, delta: f32) {
        let Some(mut players) = world.storage_mut::<AudioPlayer>() else {
            self.players.clear();
            return;
        };
        let infos = world.storage::<NodeInfo>();
        let transforms = world.storage::<Transform>();
        let mut live = Vec::new();
        for (entity, player) in players.iter_mut() {
            if !infos
                .as_ref()
                .and_then(|i| i.get(entity))
                .is_some_and(|i| i.in_tree)
            {
                // Out of the tree: silent, and play() requests are dropped as in Godot.
                player.command = None;
                player.playing = false;
                continue;
            }
            live.push(entity);
            let position = transforms
                .as_ref()
                .and_then(|t| t.get(entity))
                .map_or(Vec3::ZERO, |t| t.global.translation.into());
            let state = self.players.entry(entity).or_default();
            if player.autoplay && !state.autoplayed {
                state.autoplayed = true;
                player.command.get_or_insert(Command::Play);
            }
            // A player turned 2D/3D or moved to another bus restarts on the new route.
            let rerouted = state.positional != player.positional || state.bus != player.bus;
            let current = state.voice;
            let voice = current.and_then(|id| self.voices.iter().position(|v| v.id == id));
            if rerouted && voice.is_some() && player.command.is_none() {
                player.command = Some(Command::Play);
            }
            match player.command.take() {
                Some(Command::Play) => {
                    if let Some(i) = voice {
                        self.voices[i].stop(Duration::ZERO);
                    }
                    let started = self.start_player(entity, player, position);
                    let state = self.players.entry(entity).or_default();
                    state.voice = started.map(|v| v.0);
                    state.positional = player.positional.clone();
                    state.bus = player.bus.clone();
                    player.playing = started.is_some();
                }
                Some(Command::Stop) => {
                    if let Some(i) = voice {
                        self.voices[i].stop(STOP_FADE);
                    }
                    self.players.entry(entity).or_default().voice = None;
                }
                None => {
                    if let Some(i) = voice {
                        let v = &mut self.voices[i];
                        v.velocity = velocity(v.position, position, delta);
                        v.position = position;
                        v.set_player_base(player_base(player));
                    }
                }
            }
        }
        drop(players);
        // Freed or detached players stop.
        self.players.retain(|entity, state| {
            let keep = live.contains(entity);
            if !keep {
                if let Some(id) = state.voice {
                    if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
                        v.stop(Duration::ZERO);
                    }
                }
            }
            keep
        });
    }

    fn start_player(
        &mut self,
        entity: Entity,
        player: &AudioPlayer,
        position: Vec3,
    ) -> Option<VoiceId> {
        let spatial = player.positional.clone().map(|a| (a, position));
        let id = if let Some(event) = &player.event {
            let event = event.clone();
            self.start_event(
                &event,
                spatial,
                Some(entity),
                player.volume_db,
                player.pitch_scale,
                Some((&player.bus, player.looping)),
            )
        } else {
            let stream = player.stream.as_ref()?;
            let (attenuation, position) = match spatial {
                Some((a, p)) => (Some(a), p),
                None => (None, Vec3::ZERO),
            };
            self.start(Start {
                stream,
                stream_end: None,
                volume_db: player.volume_db,
                rate: f64::from(player.pitch_scale.max(MIN_PITCH_SCALE)),
                looping: player.looping,
                bus: &player.bus,
                attenuation,
                position,
                owner: Some(entity),
                event: None,
                priority: 64,
            })
        }?;
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id.0) {
            // Already part of what it started with.
            (v.player_volume_db, v.player_rate) = player_base(player);
        }
        Some(id)
    }

    fn update_voices(&mut self, delta: f32) {
        let generation = self.mixer.generation;
        let listener = self.listener_pose.0;
        let listener_velocity = self.listener_velocity;
        let occlusion_step = if OCCLUSION_TIME > 0.0 {
            (delta / OCCLUSION_TIME).min(1.0)
        } else {
            1.0
        };
        for v in &mut self.voices {
            if v.generation != generation {
                v.stopped = true;
            }
            if v.stopped {
                continue;
            }
            if v.volume_db != v.applied_volume_db {
                v.sound.set_volume(v.volume_db);
                v.applied_volume_db = v.volume_db;
            }
            let mut rate = v.rate;
            if let Some(attenuation) = &v.attenuation {
                let offset = v.position - listener;
                let distance = offset.length();
                v.distance_gain = attenuation.gain(distance);
                v.occlusion += (v.occlusion_target - v.occlusion) * occlusion_step;
                if attenuation.doppler > 0.0 && distance > 1e-3 {
                    let n = offset / distance;
                    let k = attenuation.doppler;
                    let toward_listener = (listener_velocity * k).dot(n);
                    let toward_source = (v.velocity * k).dot(n);
                    let factor = (SPEED_OF_SOUND + toward_listener)
                        / (SPEED_OF_SOUND + toward_source).max(1.0);
                    rate *= f64::from(factor.clamp(0.5, 2.0));
                }
                let track_db = gain_to_db(v.distance_gain) + OCCLUDED_DB * v.occlusion;
                if let Some(track) = &mut v.track {
                    if (track_db - v.applied_track_db).abs() > 0.05 {
                        track.set_volume(
                            decibels(track_db),
                            Tween {
                                duration: Duration::from_millis(30),
                                ..Tween::default()
                            },
                        );
                        v.applied_track_db = track_db;
                    }
                    track.set_position(mint_vec(v.position), Tween::default());
                }
                if let Some(cutoff) = &v.cutoff {
                    let open = crate::dsp::biquad::VoiceFilter::OPEN_HZ;
                    cutoff.store(crate::effect::lerp_log(
                        open,
                        OCCLUDED_CUTOFF_HZ,
                        v.occlusion,
                    ));
                }
                // Looping sounds out of earshot wait, paused, for the listener to return.
                let out_of_range = v.distance_gain <= 0.0;
                if v.looping && out_of_range != v.paused_out_of_range {
                    if out_of_range {
                        v.sound.pause();
                    } else {
                        v.sound.resume();
                    }
                    v.paused_out_of_range = out_of_range;
                }
            }
            if (rate - v.applied_rate).abs() > 1e-4 {
                v.sound.set_playback_rate(rate);
                v.applied_rate = rate;
            }
            if let Some((end, fade)) = v.stream_end {
                if !v.looping && v.sound.position() >= end - f64::from(fade) {
                    v.sound.stop(Duration::from_secs_f32(fade.max(0.0)));
                    v.stream_end = None;
                }
            }
        }
    }

    /// Mixes elapsed time on the silent backend so playback progresses.
    // Without the `device` feature `Silent` is the only variant.
    #[allow(irrefutable_let_patterns)]
    fn advance(&mut self, delta: f32) {
        if let Output::Silent {
            manager,
            pending_frames,
        } = &mut self.output
        {
            *pending_frames += f64::from(delta.max(0.0)) * f64::from(SILENT_SAMPLE_RATE);
            while *pending_frames >= MIX_CHUNK as f64 {
                let backend = manager.backend_mut();
                backend.on_start_processing();
                backend.process();
                *pending_frames -= MIX_CHUNK as f64;
            }
        }
    }

    /// Drops voices that ended, updates players' `is_playing` and reports the ones that
    /// ended on their own.
    fn poll(&mut self, world: Option<&World>) {
        let mut ended = Vec::new();
        self.voices.retain_mut(|v| {
            v.sound.report_errors();
            let done = v.sound.state() == PlaybackState::Stopped
                || (v.stopped && v.generation != self.mixer.generation);
            if done {
                ended.push((v.id, v.owner, !v.stopped));
            }
            !done
        });
        if ended.is_empty() {
            return;
        }
        let Some(mut players) = world.and_then(|w| w.storage_mut::<AudioPlayer>()) else {
            return;
        };
        for (id, owner, natural) in ended {
            let Some(entity) = owner else { continue };
            let Some(state) = self.players.get_mut(&entity) else {
                continue;
            };
            if state.voice != Some(id) {
                continue;
            }
            state.voice = None;
            if let Some(player) = players.get_mut(entity) {
                if player.command.is_none() && player.playing {
                    player.playing = false;
                    if natural {
                        self.finished.push(NodeHandle::from_entity(entity));
                    }
                }
            }
        }
    }
}

/// A player's volume and pitch, which apply live on top of what its event chose.
fn player_base(player: &AudioPlayer) -> (f32, f64) {
    (
        player.volume_db,
        f64::from(player.pitch_scale.max(MIN_PITCH_SCALE)),
    )
}

impl Voice {
    /// Swaps the player's volume and rate inside `volume_db` and `rate` for new ones.
    fn set_player_base(&mut self, (volume_db, rate): (f32, f64)) {
        self.volume_db += volume_db - self.player_volume_db;
        self.player_volume_db = volume_db;
        if self.player_rate > 0.0 {
            self.rate *= rate / self.player_rate;
        }
        self.player_rate = rate;
    }
}

fn velocity(from: Vec3, to: Vec3, delta: f32) -> Vec3 {
    if delta > 1e-4 {
        let v = (to - from) / delta;
        if v.is_finite() {
            return v;
        }
    }
    Vec3::ZERO
}

impl Default for Audio {
    fn default() -> Self {
        Self::new(AudioOutput::default())
    }
}

impl fmt::Debug for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Audio")
            .field("silent", &self.is_silent())
            .field("voices", &self.voices.len())
            .field("master_volume_db", &self.master_volume_db)
            .finish_non_exhaustive()
    }
}

/// Updates [`Audio`] once per frame with [`Time::delta`]. The engine runs it in
/// `PostUpdate`, after transforms are propagated.
pub fn audio_system(world: &mut World) {
    let world: &World = world;
    let delta = world.resource::<Time>().map_or(0.0, |time| time.delta);
    if let Some(mut audio) = world.resource_mut::<Audio>() {
        audio.update(world, delta);
    }
}

fn manager_settings<B: Backend>(
    backend_settings: B::Settings,
    main: MainTrackBuilder,
) -> AudioManagerSettings<B> {
    AudioManagerSettings {
        capacities: Capacities {
            // Buses plus 3D voices played on the master.
            sub_track_capacity: 512,
            send_track_capacity: 32,
            ..Capacities::default()
        },
        main_track_builder: main.sound_capacity(512),
        internal_buffer_size: MIX_CHUNK,
        backend_settings,
    }
}

#[cfg(feature = "device")]
fn open_device(main: MainTrackBuilder) -> Option<Output> {
    match AudioManager::<kira::DefaultBackend>::new(manager_settings(Default::default(), main)) {
        Ok(manager) => {
            info!("audio output opened");
            Some(Output::Device(Box::new(manager)))
        }
        Err(err) => {
            warn!("cannot open the audio device ({err}); continuing without sound");
            None
        }
    }
}

#[cfg(not(feature = "device"))]
fn open_device(_: MainTrackBuilder) -> Option<Output> {
    info!("built without the `device` feature; audio is silent");
    None
}

fn silent_output(main: MainTrackBuilder) -> Output {
    let settings = manager_settings(
        MockBackendSettings {
            sample_rate: SILENT_SAMPLE_RATE,
        },
        main,
    );
    let manager = AudioManager::<MockBackend>::new(settings)
        .expect("the mock audio backend cannot fail to start");
    Output::Silent {
        manager: Box::new(manager),
        pending_frames: 0.0,
    }
}

/// A 3D voice's track: kira pans it, the voice applies the distance gain itself.
fn spatial_builder(attenuation: &Attenuation) -> SpatialTrackBuilder {
    let min_distance = attenuation.min_distance.max(0.0);
    let spread = if attenuation.spread.is_finite() {
        attenuation.spread.clamp(0.0, 1.0)
    } else {
        0.0
    };
    SpatialTrackBuilder::new()
        .distances(SpatialTrackDistances {
            min_distance,
            max_distance: attenuation.max_distance.max(min_distance + 1e-3),
        })
        .attenuation_function(None)
        .spatialization_strength(1.0 - spread)
        .sound_capacity(2)
        .sub_track_capacity(1)
}

/// Sets up one playback of `stream`; `None` (with a warning) if a streaming file cannot be
/// opened.
fn configure(stream: &AudioStream, volume_db: f32, rate: f64, looping: bool) -> Option<Playback> {
    let cut = stream.cut;
    let to_region = |(from, to): (f64, Option<f64>)| Region {
        start: PlaybackPosition::Seconds(from),
        end: to.map_or(EndPosition::EndOfAudio, |t| {
            EndPosition::Custom(PlaybackPosition::Seconds(t))
        }),
    };
    let region = looping.then(|| cut.loop_region.map_or(Region::from(..), to_region));
    let volume = decibels(volume_db + cut.volume_db);
    let fade_in = (cut.fade_in > 0.0).then(|| Tween {
        duration: Duration::from_secs_f32(cut.fade_in),
        ..Tween::default()
    });
    match &stream.data {
        StreamData::Decoded(data) => Some(Playback::Decoded(
            data.volume(volume)
                .playback_rate(rate)
                .loop_region(region)
                .fade_in_tween(fade_in),
        )),
        StreamData::Streaming { bytes, .. } => match streaming_data(bytes) {
            Ok(data) => {
                let data = match cut.slice {
                    Some(slice) => data.slice(to_region(slice)),
                    None => data,
                };
                Some(Playback::Streaming(
                    data.volume(volume)
                        .playback_rate(rate)
                        .loop_region(region)
                        .fade_in_tween(fade_in),
                ))
            }
            Err(err) => {
                warn!("cannot play sound: {err}");
                None
            }
        },
    }
}

/// A NaN position (a body that blew up, `play_at(NaN)`) would make the track's gain NaN and
/// poison the mix: non-finite vectors become zero.
fn mint_vec(v: Vec3) -> mint::Vector3<f32> {
    let v = if v.is_finite() { v } else { Vec3::ZERO };
    mint::Vector3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn mint_quat(q: Quat) -> mint::Quaternion<f32> {
    mint::Quaternion {
        v: mint_vec(Vec3::new(q.x, q.y, q.z)),
        s: q.w,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ClipDesc, SoundEventDesc};
    use crate::mixer::{BusDesc, SnapshotDesc};
    use crate::AudioNodes;
    use ab_scene::SceneTree;

    fn main_track_sounds(tree: &SceneTree) -> usize {
        let mut audio = tree.world().resource_mut::<Audio>().unwrap();
        with_manager!(&mut audio.output, m => m.main_track().num_sounds())
    }

    fn bus_sounds(audio: &mut Audio, bus: &str) -> usize {
        audio.mixer.track_mut(Some(bus)).unwrap().num_sounds()
    }

    fn frame(tree: &mut SceneTree) {
        tree.process(1.0 / 60.0);
        let world = tree.world();
        world
            .resource_mut::<Audio>()
            .unwrap()
            .update(world, 1.0 / 60.0);
    }

    fn run(audio: &mut Audio, seconds: f32) {
        let world = World::new();
        let frames = (seconds * 60.0).ceil() as usize;
        for _ in 0..frames {
            audio.update(&world, 1.0 / 60.0);
        }
    }

    /// An event of one `seconds`-long clip.
    fn event(seconds: f32, edit: impl FnOnce(&mut SoundEventDesc)) -> SoundEvent {
        let mut desc = SoundEventDesc {
            clips: vec![ClipDesc::new("beep.wav")],
            ..SoundEventDesc::default()
        };
        edit(&mut desc);
        let samples = vec![0.1; (8000.0 * seconds) as usize];
        SoundEvent::new(
            "beep.absound",
            desc,
            vec![Some(AudioStream::from_samples(8000, &samples))],
        )
    }

    #[test]
    fn becoming_positional_stops_the_2d_sound() {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Audio::silent());
        let music = AudioStream::from_samples(8000, &[0.0; 4000]);
        let node =
            tree.create_audio_stream_player("Music", AudioPlayer::new(music).with_looping(true));
        tree.add_child(tree.root(), node).unwrap();
        tree.component_mut::<AudioPlayer>(node).unwrap().play();
        for _ in 0..3 {
            frame(&mut tree);
        }
        assert_eq!(main_track_sounds(&tree), 1);
        tree.component_mut::<AudioPlayer>(node).unwrap().positional = Some(Attenuation::default());
        for _ in 0..10 {
            frame(&mut tree);
        }
        assert!(tree.component::<AudioPlayer>(node).unwrap().is_playing());
        assert_eq!(
            main_track_sounds(&tree),
            0,
            "the looping 2D sound must not keep playing next to the 3D one"
        );
    }

    #[test]
    fn a_player_reports_finished_once_its_sound_ends() {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Audio::silent());
        tree.world_mut().insert_resource(SignalQueue::default());
        let beep = AudioStream::from_samples(8000, &[0.0; 800]);
        let node = tree.create_audio_stream_player("Beep", AudioPlayer::new(beep).with_bus("UI"));
        tree.add_child(tree.root(), node).unwrap();
        tree.component_mut::<AudioPlayer>(node).unwrap().play();
        frame(&mut tree);
        assert_eq!(
            bus_sounds(&mut tree.world().resource_mut::<Audio>().unwrap(), "UI"),
            1,
            "on its bus"
        );
        let mut reported = false;
        for _ in 0..30 {
            frame(&mut tree);
            reported |= tree.world().resource::<Audio>().unwrap().finished() == [node];
        }
        assert!(reported);
        assert!(!tree.component::<AudioPlayer>(node).unwrap().is_playing());
    }

    #[test]
    fn events_play_on_their_bus_and_steal_their_oldest_voice() {
        let mut audio = Audio::silent();
        let shot = event(1.0, |d| {
            d.bus = "SFX".into();
            d.max_voices = 2;
        });
        let first = audio.play_event(&shot).unwrap();
        run(&mut audio, 0.05);
        let second = audio.play_event(&shot).unwrap();
        let third = audio.play_event(&shot).unwrap();
        assert!(!audio.is_voice_playing(first), "the oldest made room");
        assert!(audio.is_voice_playing(second) && audio.is_voice_playing(third));
        assert_eq!(audio.voice_stats().stolen, 1);
        run(&mut audio, 0.1);
        assert_eq!(bus_sounds(&mut audio, "SFX"), 2);
    }

    #[test]
    fn a_full_event_without_stealing_refuses_and_cooldown_holds_back() {
        let mut audio = Audio::silent();
        let one = event(1.0, |d| {
            d.max_voices = 1;
            d.steal = StealMode::None;
        });
        assert!(audio.play_event(&one).is_some());
        assert!(audio.play_event(&one).is_none());

        let rapid = event(0.05, |d| d.cooldown = 0.2);
        assert!(audio.play_event(&rapid).is_some());
        assert!(audio.play_event(&rapid).is_none(), "in cooldown");
        run(&mut audio, 0.25);
        assert!(audio.play_event(&rapid).is_some());
        assert_eq!(audio.voice_stats().refused, 2);
    }

    #[test]
    fn past_the_voice_limit_only_more_important_sounds_get_in() {
        let mut audio = Audio::silent();
        audio.configure_mixer(MixerSettings {
            max_voices: 2,
            ..MixerSettings::default()
        });
        let ambient = event(2.0, |d| d.priority = 200);
        let gunshot = event(2.0, |d| d.priority = 10);
        audio.play_event(&ambient).unwrap();
        audio.play_event(&ambient).unwrap();
        assert!(
            audio.play_event(&ambient).is_none(),
            "full, nothing less important"
        );
        assert!(
            audio.play_event(&gunshot).is_some(),
            "an ambience voice made room"
        );
        assert_eq!(audio.voice_stats().real, 2);
    }

    #[test]
    fn far_one_shots_are_skipped_and_far_loops_wait_paused() {
        let mut audio = Audio::silent();
        let far = Vec3::new(100.0, 0.0, 0.0);
        let thud = event(0.5, |d| {
            d.spatial = Some(Attenuation {
                max_distance: 30.0,
                ..Attenuation::default()
            });
        });
        assert!(audio.play_event_at(&thud, far).is_none());

        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(audio);
        let hum = AudioStream::from_samples(8000, &[0.1; 800]);
        let node = tree.create_audio_stream_player3d(
            "Hum",
            AudioPlayer::positional(hum)
                .with_looping(true)
                .with_attenuation(Attenuation {
                    max_distance: 30.0,
                    ..Attenuation::default()
                }),
        );
        tree.add_child(tree.root(), node).unwrap();
        tree.set_position(node, far).unwrap();
        tree.component_mut::<AudioPlayer>(node).unwrap().play();
        for _ in 0..3 {
            frame(&mut tree);
        }
        let stats = tree.world().resource::<Audio>().unwrap().voice_stats();
        assert_eq!((stats.real, stats.virtual_voices), (0, 1));
        tree.set_position(node, Vec3::new(5.0, 0.0, 0.0)).unwrap();
        for _ in 0..3 {
            frame(&mut tree);
        }
        let stats = tree.world().resource::<Audio>().unwrap().voice_stats();
        assert_eq!((stats.real, stats.virtual_voices), (1, 0));
        assert!(tree.component::<AudioPlayer>(node).unwrap().is_playing());
    }

    #[test]
    fn snapshots_fade_in_and_out_over_their_times() {
        let mut audio = Audio::silent();
        audio.configure_mixer(MixerSettings {
            snapshots: vec![SnapshotDesc {
                name: "Pause".into(),
                fade_in: 0.5,
                fade_out: 0.25,
                ..SnapshotDesc::default()
            }],
            ..MixerSettings::default()
        });
        assert!(!audio.set_snapshot("Nope", true));
        assert!(audio.set_snapshot("Pause", true));
        run(&mut audio, 0.25);
        assert!((audio.snapshot_level("Pause") - 0.5).abs() < 0.05);
        run(&mut audio, 0.5);
        assert_eq!(audio.snapshot_level("Pause"), 1.0);
        audio.set_snapshot("Pause", false);
        run(&mut audio, 0.3);
        assert_eq!(audio.snapshot_level("Pause"), 0.0);
    }

    #[test]
    fn a_new_bus_layout_restarts_players_on_the_new_buses() {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Audio::silent());
        let music = AudioStream::from_samples(8000, &[0.1; 8000]);
        let node = tree.create_audio_stream_player(
            "Music",
            AudioPlayer::new(music).with_looping(true).with_bus("Music"),
        );
        tree.add_child(tree.root(), node).unwrap();
        tree.component_mut::<AudioPlayer>(node).unwrap().play();
        frame(&mut tree);
        {
            let mut audio = tree.world().resource_mut::<Audio>().unwrap();
            let mut settings = audio.mixer_settings().clone();
            settings.buses.push(BusDesc::new("Extra"));
            audio.configure_mixer(settings);
        }
        for _ in 0..3 {
            frame(&mut tree);
        }
        // The old voice is gone with its bus; the player stopped and reports it.
        assert!(!tree.component::<AudioPlayer>(node).unwrap().is_playing());
        tree.component_mut::<AudioPlayer>(node).unwrap().play();
        frame(&mut tree);
        assert_eq!(
            bus_sounds(&mut tree.world().resource_mut::<Audio>().unwrap(), "Music"),
            1
        );
    }

    #[test]
    fn occlusion_requests_list_3d_voices_that_want_it() {
        let mut audio = Audio::silent();
        let behind_wall = event(1.0, |d| {
            d.spatial = Some(Attenuation {
                occlusion: true,
                ..Attenuation::default()
            });
        });
        let id = audio
            .play_event_at(&behind_wall, Vec3::new(3.0, 0.0, 0.0))
            .unwrap();
        assert_eq!(audio.occlusion_requests(), [(id, Vec3::new(3.0, 0.0, 0.0))]);
        audio.set_occlusion(id, 1.0);
        run(&mut audio, 0.5);
        let voice = audio.voices.iter().find(|v| v.id == id.0).unwrap();
        assert!(voice.occlusion > 0.95);
        assert!(voice.cutoff.as_ref().unwrap().load() < 1_000.0);
    }
}
