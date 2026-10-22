use crate::components::{Attenuation, AudioListener3D, AudioPlayer, Command};
use crate::stream::AudioStream;
use ab_core::{Affine3A, Quat, Time, Vec3};
use ab_ecs::{Entity, World};
use ab_scene::{NodeHandle, NodeInfo, Signal, SignalQueue, Transform};
use kira::backend::mock::{MockBackend, MockBackendSettings};
use kira::backend::Backend;
use kira::listener::{ListenerHandle, ListenerId};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::{PlaybackState, Region};
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle};
use kira::{AudioManager, AudioManagerSettings, Capacities, Tween};
use std::collections::{HashMap, HashSet};
use std::fmt;
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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Sample rate of the silent backend: low, since nothing is heard.
const SILENT_SAMPLE_RATE: u32 = 8000;
/// Frames the mixer renders per process call (kira's default internal buffer size).
const MIX_CHUNK: usize = 128;

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

/// Playback state of one [`AudioPlayer`] node.
#[derive(Default)]
struct Voice {
    sound: Option<StaticSoundHandle>,
    /// Spatial track of a positional player, with the attenuation it was built with.
    track: Option<(SpatialTrackHandle, Attenuation)>,
    position: Vec3,
    volume_db: f32,
    pitch_scale: f32,
    autoplayed: bool,
}

impl Voice {
    fn stop(&mut self) {
        if let Some(mut sound) = self.sound.take() {
            sound.stop(Tween::default());
        }
    }
}

/// ECS resource owning the audio output (kira), the listener and every playing sound.
///
/// [`audio_system`] syncs it with the scene once per frame: it moves the listener to the
/// current [`AudioListener3D`], starts, stops and updates [`AudioPlayer`] nodes, and follows
/// 3D players around. Fire-and-forget sounds go through [`Audio::play`] and
/// [`Audio::play_at`].
pub struct Audio {
    output: Output,
    listener: ListenerHandle,
    listener_pose: (Vec3, Quat),
    voices: HashMap<Entity, Voice>,
    finished: Vec<NodeHandle>,
    master_volume_db: f32,
}

impl Audio {
    pub fn new(output: AudioOutput) -> Self {
        let output = match output {
            AudioOutput::Device => open_device().unwrap_or_else(silent_output),
            AudioOutput::Silent => silent_output(),
        };
        let mut output = output;
        let listener = with_manager!(&mut output, m => m.add_listener(mint_vec(Vec3::ZERO), mint_quat(Quat::IDENTITY)))
            .expect("a fresh audio manager has room for a listener");
        Self {
            output,
            listener,
            listener_pose: (Vec3::ZERO, Quat::IDENTITY),
            voices: HashMap::new(),
            finished: Vec::new(),
            master_volume_db: 0.0,
        }
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

    /// Gain applied to everything, in decibels.
    pub fn set_master_volume_db(&mut self, volume_db: f32) {
        self.master_volume_db = volume_db;
        with_manager!(&mut self.output, m => m.main_track().set_volume(volume_db, Tween::default()));
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
        let data = configure(stream, options.volume_db, options.pitch_scale, false);
        if let Err(err) = with_manager!(&mut self.output, m => m.play(data)) {
            warn!("cannot play sound: {err}");
        }
    }

    /// Plays a one-shot at a global position (impacts, footsteps, explosions).
    pub fn play_at(&mut self, stream: &AudioStream, position: Vec3, options: SoundOptions) {
        let data = configure(stream, options.volume_db, options.pitch_scale, false);
        let builder = spatial_builder(options.attenuation).persist_until_sounds_finish(true);
        let listener = self.listener.id();
        let track = with_manager!(&mut self.output, m => m.add_spatial_sub_track(listener, mint_vec(position), builder));
        match track {
            // The track outlives its handle until the sound is done.
            Ok(mut track) => {
                if let Err(err) = track.play(data) {
                    warn!("cannot play sound: {err}");
                }
            }
            Err(err) => warn!("cannot play positional sound: {err}"),
        }
    }

    /// Syncs listener and players with the scene, then advances the silent backend by
    /// `delta` seconds. Normally run by [`audio_system`].
    pub fn update(&mut self, world: &World, delta: f32) {
        self.finished.clear();
        self.update_listener(world);
        self.update_players(world);
        self.advance(delta);
        self.poll_players(world);
        if let Some(mut queue) = world.resource_mut::<SignalQueue>() {
            for &node in &self.finished {
                queue.push(Signal::new(node, AudioPlayer::FINISHED));
            }
        }
    }

    fn update_listener(&mut self, world: &World) {
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
        if pose != self.listener_pose {
            self.listener_pose = pose;
            self.listener
                .set_position(mint_vec(pose.0), Tween::default());
            self.listener
                .set_orientation(mint_quat(pose.1), Tween::default());
        }
    }

    fn update_players(&mut self, world: &World) {
        let mut live = HashSet::new();
        if let (Some(mut players), Some(infos)) = (
            world.storage_mut::<AudioPlayer>(),
            world.storage::<NodeInfo>(),
        ) {
            let transforms = world.storage::<Transform>();
            let listener = self.listener.id();
            for (entity, player) in players.iter_mut() {
                if !infos.get(entity).is_some_and(|i| i.in_tree) {
                    // Out of the tree: silent, and play() requests are dropped as in Godot.
                    player.command = None;
                    player.playing = false;
                    continue;
                }
                live.insert(entity);
                let position = transforms
                    .as_ref()
                    .and_then(|t| t.get(entity))
                    .map_or(Vec3::ZERO, |t| t.global.translation.into());
                let voice = self.voices.entry(entity).or_default();
                sync_track(&mut self.output, listener, voice, player, position);

                if player.autoplay && !voice.autoplayed {
                    voice.autoplayed = true;
                    player.command.get_or_insert(Command::Play);
                }
                match player.command.take() {
                    Some(Command::Play) => {
                        voice.stop();
                        start(&mut self.output, voice, player);
                    }
                    Some(Command::Stop) => voice.stop(),
                    None => {
                        if let Some(sound) = &mut voice.sound {
                            if voice.volume_db != player.volume_db {
                                sound.set_volume(player.volume_db, Tween::default());
                                voice.volume_db = player.volume_db;
                            }
                            if voice.pitch_scale != player.pitch_scale {
                                sound.set_playback_rate(
                                    f64::from(player.pitch_scale),
                                    Tween::default(),
                                );
                                voice.pitch_scale = player.pitch_scale;
                            }
                        }
                    }
                }
            }
        }
        // Freed or detached players stop.
        self.voices.retain(|entity, voice| {
            let keep = live.contains(entity);
            if !keep {
                voice.stop();
            }
            keep
        });
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

    /// Updates `is_playing` and reports streams that ended on their own.
    fn poll_players(&mut self, world: &World) {
        let Some(mut players) = world.storage_mut::<AudioPlayer>() else {
            return;
        };
        for (entity, voice) in &mut self.voices {
            let Some(player) = players.get_mut(*entity) else {
                continue;
            };
            let ended = voice
                .sound
                .as_ref()
                .is_some_and(|s| s.state() == PlaybackState::Stopped);
            if ended {
                voice.sound = None;
            }
            if voice.sound.is_none() && player.command.is_none() && player.playing {
                player.playing = false;
                if ended {
                    self.finished.push(NodeHandle::from_entity(*entity));
                }
            }
        }
    }
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
            .finish()
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

fn manager_settings<B: Backend>(backend_settings: B::Settings) -> AudioManagerSettings<B> {
    AudioManagerSettings {
        capacities: Capacities {
            // One per 3D player plus in-flight positional one-shots.
            sub_track_capacity: 512,
            ..Capacities::default()
        },
        main_track_builder: Default::default(),
        internal_buffer_size: MIX_CHUNK,
        backend_settings,
    }
}

#[cfg(feature = "device")]
fn open_device() -> Option<Output> {
    match AudioManager::<kira::DefaultBackend>::new(manager_settings(Default::default())) {
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
fn open_device() -> Option<Output> {
    info!("built without the `device` feature; audio is silent");
    None
}

fn silent_output() -> Output {
    let settings = manager_settings(MockBackendSettings {
        sample_rate: SILENT_SAMPLE_RATE,
    });
    let manager = AudioManager::<MockBackend>::new(settings)
        .expect("the mock audio backend cannot fail to start");
    Output::Silent {
        manager: Box::new(manager),
        pending_frames: 0.0,
    }
}

fn spatial_builder(attenuation: Attenuation) -> SpatialTrackBuilder {
    let min_distance = attenuation.min_distance.max(0.0);
    SpatialTrackBuilder::new()
        .distances(SpatialTrackDistances {
            min_distance,
            max_distance: attenuation.max_distance.max(min_distance + 1e-3),
        })
        .sound_capacity(8)
        .sub_track_capacity(1)
}

/// Creates, moves or removes the spatial track of a player as its settings require. A track
/// rebuilt while a sound plays restarts the sound on the new track.
fn sync_track(
    output: &mut Output,
    listener: ListenerId,
    voice: &mut Voice,
    player: &mut AudioPlayer,
    position: Vec3,
) {
    let current = voice.track.as_ref().map(|(_, attenuation)| *attenuation);
    if current == player.positional {
        if let Some((track, _)) = &mut voice.track {
            if position != voice.position {
                track.set_position(mint_vec(position), Tween::default());
            }
        }
        voice.position = position;
        return;
    }
    // Dropping a track handle removes the track and its sounds.
    let was_playing = voice.sound.take().is_some();
    voice.track = None;
    voice.position = position;
    if let Some(attenuation) = player.positional {
        let builder = spatial_builder(attenuation);
        match with_manager!(output, m => m.add_spatial_sub_track(listener, mint_vec(position), builder))
        {
            Ok(track) => voice.track = Some((track, attenuation)),
            Err(err) => warn!("cannot create a 3D audio track: {err}"),
        }
    }
    if was_playing && player.playing {
        player.command.get_or_insert(Command::Play);
    }
}

/// Starts the player's stream on its track (3D) or the main track.
fn start(output: &mut Output, voice: &mut Voice, player: &mut AudioPlayer) {
    let Some(stream) = &player.stream else {
        player.playing = false;
        return;
    };
    let data = configure(stream, player.volume_db, player.pitch_scale, player.looping);
    let result = match &mut voice.track {
        Some((track, _)) => track.play(data),
        None if player.positional.is_some() => {
            // Track creation failed (already warned).
            player.playing = false;
            return;
        }
        None => with_manager!(output, m => m.play(data)),
    };
    match result {
        Ok(sound) => {
            voice.sound = Some(sound);
            voice.volume_db = player.volume_db;
            voice.pitch_scale = player.pitch_scale;
            player.playing = true;
        }
        Err(err) => {
            warn!("cannot play sound: {err}");
            player.playing = false;
        }
    }
}

fn configure(
    stream: &AudioStream,
    volume_db: f32,
    pitch_scale: f32,
    looping: bool,
) -> StaticSoundData {
    stream
        .data
        .volume(volume_db)
        .playback_rate(f64::from(pitch_scale.max(0.01)))
        .loop_region(looping.then(|| Region::from(..)))
}

fn mint_vec(v: Vec3) -> mint::Vector3<f32> {
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
