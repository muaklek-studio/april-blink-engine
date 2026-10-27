use crate::error::AssetError;
use crate::gltf_loader::{self, GltfScene};
use crate::image_loader::decode_image;
use crate::loader::{AssetRequest, LoadStatus, Loader, Outcome};
use crate::source::{normalize_path, AssetSource, FileSystemSource};
use ab_animation::AnimationLibrary;
use ab_audio::{AudioStream, SoundEvent, SoundEventDesc};
use ab_rendering::{ColorSpace, RenderAssets, SamplerSettings, Texture, TextureHandle};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// Audio files at least this large (bytes) and long ([`STREAMING_AUDIO_MIN_SECONDS`]) load
/// as streaming sounds, decoded while they play ([`AudioStream::streaming`]): music and long
/// ambience. Others (effects, even large uncompressed ones) are decoded into memory up
/// front, so they start at once every time they play.
pub const STREAMING_AUDIO_MIN_BYTES: usize = 1024 * 1024;

/// See [`STREAMING_AUDIO_MIN_BYTES`].
pub const STREAMING_AUDIO_MIN_SECONDS: f32 = 10.0;

/// Loads assets through an [`AssetSource`] and caches them by normalised path.
///
/// Stored as an ECS resource. Loaded resources are registered in the world's
/// [`RenderAssets`] / [`AnimationLibrary`], so callers pass those in.
///
/// Two ways to load:
/// * `load_*` reads and decodes on the calling thread and returns the resource.
/// * `request_*` starts reading and decoding on a background thread and returns at once;
///   [`AssetServer::poll`] (run every frame by the engine) registers what has finished, and
///   from then on `load_*` returns it from the cache without waiting. Request what a level
///   needs ahead of time (a loading screen, the next area) and wait on
///   [`AssetServer::pending_loads`] or [`AssetServer::is_pending`].
pub struct AssetServer {
    source: Arc<dyn AssetSource>,
    loader: Loader,
    /// Requests in flight, with the id of the latest one (an evicted path's earlier
    /// request is ignored when it comes back).
    pending: HashMap<AssetRequest, u64>,
    next_request: u64,
    /// Background loads that failed, and why (until evicted or requested again).
    failed: HashMap<AssetRequest, String>,
    scenes: HashMap<String, Arc<GltfScene>>,
    /// glTF files that failed to load, and why: not decoded again on every spawn (each try
    /// would upload its textures again) until the file changes ([`AssetServer::evict`]).
    failed_scenes: HashMap<String, String>,
    textures: HashMap<(String, ColorSpace), TextureHandle>,
    audio: HashMap<String, AudioStream>,
    sound_events: HashMap<String, SoundEvent>,
    /// `RenderAssets::id` the cached textures and scenes were registered in, and
    /// `AnimationLibrary::id` of the scenes' clips. Loading into other ones drops the stale
    /// handles instead of returning them.
    render_id: Option<u64>,
    library_id: Option<u64>,
}

impl AssetServer {
    pub fn new(source: impl AssetSource) -> Self {
        Self {
            source: Arc::new(source),
            loader: Loader::new(),
            pending: HashMap::new(),
            next_request: 0,
            failed: HashMap::new(),
            scenes: HashMap::new(),
            textures: HashMap::new(),
            audio: HashMap::new(),
            sound_events: HashMap::new(),
            failed_scenes: HashMap::new(),
            render_id: None,
            library_id: None,
        }
    }

    /// Forgets cached handles issued by a different `RenderAssets` than `render`.
    fn bind_render(&mut self, render: &RenderAssets) {
        if self.render_id.replace(render.id()) != Some(render.id()) {
            self.textures.clear();
            self.scenes.clear();
            self.failed_scenes.clear();
        }
    }

    /// Forgets cached scenes whose clips live in a different library than `library`.
    fn bind_library(&mut self, library: &AnimationLibrary) {
        if self.library_id.replace(library.id()) != Some(library.id()) {
            self.scenes.clear();
        }
    }

    /// Serves files from `root` on disk.
    pub fn from_directory(root: impl Into<PathBuf>) -> Self {
        Self::new(FileSystemSource::new(root))
    }

    fn normalize(path: &str) -> Result<String, AssetError> {
        normalize_path(path)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| AssetError::invalid(path, "path is empty or escapes the asset root"))
    }

    /// Raw bytes of an asset.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let path = Self::normalize(path)?;
        self.source
            .read(&path)
            .map_err(|source| AssetError::Io { path, source })
    }

    /// Sounds kept loaded: how many, and their bytes.
    pub fn audio_memory(&self) -> (usize, usize) {
        (
            self.audio.len(),
            self.audio.values().map(AudioStream::memory_bytes).sum(),
        )
    }

    /// Models (glTF scenes) kept loaded.
    pub fn loaded_models(&self) -> usize {
        self.scenes.len()
    }

    /// Forgets what was loaded from `path` (its file changed): the next load reads it again.
    pub fn evict(&mut self, path: &str) {
        if let Ok(path) = Self::normalize(path) {
            self.scenes.remove(&path);
            self.failed_scenes.remove(&path);
            self.textures.retain(|(p, _), _| *p != path);
            self.audio.remove(&path);
            // An event goes with its own file and with any of its clips.
            self.sound_events.retain(|event_path, event| {
                *event_path != path
                    && !event
                        .desc()
                        .clips
                        .iter()
                        .any(|c| Self::normalize(&c.path).is_ok_and(|clip| clip == path))
            });
            self.pending.retain(|key, _| key.path() != path);
            self.failed.retain(|key, _| key.path() != path);
        }
    }

    /// Where the bytes come from, for reading on other threads (scanning a level for what
    /// to request).
    pub fn source(&self) -> Arc<dyn AssetSource> {
        self.source.clone()
    }

    pub fn exists(&self, path: &str) -> bool {
        Self::normalize(path).is_ok_and(|p| self.source.exists(&p))
    }

    /// Loads a PNG/JPEG as a texture (cached per path and color space).
    pub fn load_texture(
        &mut self,
        render: &mut RenderAssets,
        path: &str,
        color_space: ColorSpace,
    ) -> Result<TextureHandle, AssetError> {
        let path = Self::normalize(path)?;
        self.bind_render(render);
        if let Some(&handle) = self.textures.get(&(path.clone(), color_space)) {
            return Ok(handle);
        }
        let bytes = self.read(&path)?;
        let texture = decode_image(&path, &bytes, color_space, SamplerSettings::default())?
            .with_prepared_mips();
        let handle = render
            .add_texture(texture)
            .map_err(|e| AssetError::invalid(&path, e.to_string()))?;
        self.textures.insert((path, color_space), handle);
        Ok(handle)
    }

    /// Loads a `.gltf`/`.glb` (cached per path, for the `render` / `animations` it was loaded
    /// into: passing other ones reloads, since the cached handles would not be theirs).
    /// Instantiate the result with
    /// [`instantiate_gltf`](crate::instantiate_gltf).
    pub fn load_gltf(
        &mut self,
        render: &mut RenderAssets,
        animations: &mut AnimationLibrary,
        path: &str,
    ) -> Result<Arc<GltfScene>, AssetError> {
        let path = Self::normalize(path)?;
        self.bind_render(render);
        self.bind_library(animations);
        if let Some(scene) = self.scenes.get(&path) {
            return Ok(scene.clone());
        }
        if let Some(reason) = self.failed_scenes.get(&path) {
            return Err(AssetError::invalid(&path, reason.clone()));
        }
        // A file that cannot be read yet may appear later: only bad content is remembered.
        let bytes = self.read(&path)?;
        let loaded = gltf_loader::load(&path, &bytes, self.source.as_ref(), render, animations);
        let scene = match loaded {
            Ok(scene) => Arc::new(scene),
            Err(err) => {
                self.failed_scenes.insert(path.clone(), err.to_string());
                return Err(err);
            }
        };
        info!(
            path,
            nodes = scene.nodes.len(),
            meshes = scene.meshes.len(),
            materials = scene.materials.len(),
            textures = scene.textures.len(),
            animations = scene.animations.len(),
            "glTF loaded"
        );
        self.scenes.insert(path, scene.clone());
        Ok(scene)
    }

    /// Loads and decodes a WAV/Ogg/MP3/FLAC file (cached per path; clones share samples).
    pub fn load_audio(&mut self, path: &str) -> Result<AudioStream, AssetError> {
        let path = Self::normalize(path)?;
        if let Some(stream) = self.audio.get(&path) {
            return Ok(stream.clone());
        }
        let bytes = self.read(&path)?;
        let stream = decode_audio(&path, bytes)?;
        self.audio.insert(path, stream.clone());
        Ok(stream)
    }

    /// Loads a sound event (`*.absound`) and the sound files of its clips (cached per
    /// path). A clip whose file cannot be loaded is left out, with a warning.
    pub fn load_sound_event(&mut self, path: &str) -> Result<SoundEvent, AssetError> {
        let path = Self::normalize(path)?;
        if let Some(event) = self.sound_events.get(&path) {
            return Ok(event.clone());
        }
        let bytes = self.read(&path)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| AssetError::invalid(&path, "not UTF-8 text".to_owned()))?;
        let desc = SoundEventDesc::from_toml(&text).map_err(|e| AssetError::invalid(&path, e))?;
        let streams = desc
            .clips
            .iter()
            .map(|clip| match self.load_audio(&clip.path) {
                Ok(stream) => Some(stream),
                Err(err) => {
                    warn!(event = %path, clip = %clip.path, "sound event clip skipped: {err}");
                    None
                }
            })
            .collect();
        let event = SoundEvent::new(&path, desc, streams);
        info!(path, clips = event.clip_count(), "sound event loaded");
        self.sound_events.insert(path, event.clone());
        Ok(event)
    }

    /// Starts loading a PNG/JPEG texture in the background (see the type's docs); then
    /// [`AssetServer::load_texture`] returns it without waiting. Does nothing if it is
    /// loaded or already requested.
    pub fn request_texture(
        &mut self,
        path: &str,
        color_space: ColorSpace,
    ) -> Result<(), AssetError> {
        let path = Self::normalize(path)?;
        if self.textures.contains_key(&(path.clone(), color_space)) {
            return Ok(());
        }
        let source = self.source.clone();
        self.submit(
            AssetRequest::Texture(path.clone(), color_space),
            move || {
                Outcome::Texture(read(source.as_ref(), &path).and_then(|bytes| {
                    decode_image(&path, &bytes, color_space, SamplerSettings::default())
                        .map(Texture::with_prepared_mips)
                }))
            },
        );
        Ok(())
    }

    /// Starts loading a `.gltf`/`.glb` in the background: reading, parsing and decoding its
    /// textures (see the type's docs). Does nothing if it is loaded or already requested.
    pub fn request_gltf(&mut self, path: &str) -> Result<(), AssetError> {
        let path = Self::normalize(path)?;
        if self.scenes.contains_key(&path) || self.failed_scenes.contains_key(&path) {
            return Ok(());
        }
        let source = self.source.clone();
        self.submit(AssetRequest::Gltf(path.clone()), move || {
            let source = source.as_ref();
            Outcome::Gltf(
                read(source, &path)
                    .and_then(|bytes| gltf_loader::prepare(&path, &bytes, source))
                    .map(Box::new),
            )
        });
        Ok(())
    }

    /// Starts loading an audio file in the background (see the type's docs). Does nothing
    /// if it is loaded or already requested.
    pub fn request_audio(&mut self, path: &str) -> Result<(), AssetError> {
        let path = Self::normalize(path)?;
        if self.audio.contains_key(&path) {
            return Ok(());
        }
        let source = self.source.clone();
        self.submit(AssetRequest::Audio(path.clone()), move || {
            Outcome::Audio(
                read(source.as_ref(), &path).and_then(|bytes| decode_audio(&path, bytes)),
            )
        });
        Ok(())
    }

    fn submit(&mut self, key: AssetRequest, work: impl FnOnce() -> Outcome + Send + 'static) {
        if self.pending.contains_key(&key) {
            return;
        }
        let id = self.next_request;
        self.next_request += 1;
        self.failed.remove(&key);
        self.pending.insert(key.clone(), id);
        self.loader.submit(key, id, work);
    }

    /// Registers background loads that have finished (textures and glTF resources go into
    /// `render` and `animations`), returning how many. Failures are logged; loading the
    /// path with `load_*` then reports the error. The engine calls this once per frame.
    pub fn poll(&mut self, render: &mut RenderAssets, animations: &mut AnimationLibrary) -> usize {
        let finished: Vec<_> = self.loader.finished().collect();
        if finished.is_empty() {
            return 0;
        }
        self.bind_render(render);
        self.bind_library(animations);
        let mut applied = 0;
        for (key, id, outcome) in finished {
            if self.pending.get(&key) != Some(&id) {
                continue; // Evicted while loading.
            }
            self.pending.remove(&key);
            applied += 1;
            let path = key.path().to_owned();
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(panic) => {
                    warn!(path, "background load failed: {panic}");
                    self.failed.insert(key, panic);
                    continue;
                }
            };
            let request = key.clone();
            let mut failure = None;
            match (key, outcome) {
                (AssetRequest::Texture(_, color_space), Outcome::Texture(texture)) => {
                    let key = (path.clone(), color_space);
                    if self.textures.contains_key(&key) {
                        continue; // Loaded meanwhile with `load_texture`.
                    }
                    let handle = texture.and_then(|texture| {
                        render
                            .add_texture(texture)
                            .map_err(|e| AssetError::invalid(&path, e.to_string()))
                    });
                    match handle {
                        Ok(handle) => {
                            self.textures.insert(key, handle);
                        }
                        Err(err) => failure = Some(err),
                    }
                }
                (AssetRequest::Gltf(_), Outcome::Gltf(prepared)) => {
                    if self.scenes.contains_key(&path) {
                        continue;
                    }
                    let scene = prepared.and_then(|prepared| {
                        gltf_loader::finish(*prepared, self.source.as_ref(), render, animations)
                    });
                    match scene {
                        Ok(scene) => {
                            info!(
                                path,
                                meshes = scene.meshes.len(),
                                "glTF loaded in the background"
                            );
                            self.scenes.insert(path, Arc::new(scene));
                        }
                        Err(err) => {
                            // Only bad content is remembered by `load_gltf`.
                            if !matches!(err, AssetError::Io { .. }) {
                                self.failed_scenes.insert(path, err.to_string());
                            }
                            failure = Some(err);
                        }
                    }
                }
                (AssetRequest::Audio(_), Outcome::Audio(stream)) => match stream {
                    Ok(stream) => {
                        self.audio.entry(path).or_insert(stream);
                    }
                    Err(err) => failure = Some(err),
                },
                _ => unreachable!("every request kind has its own outcome"),
            }
            if let Some(err) = failure {
                warn!("background load failed: {err}");
                self.failed.insert(request, err.to_string());
            }
        }
        applied
    }

    /// Starts loading `request` in the background ([`AssetServer::request_gltf`],
    /// [`AssetServer::request_texture`] or [`AssetServer::request_audio`]).
    pub fn request(&mut self, request: &AssetRequest) -> Result<(), AssetError> {
        match request {
            AssetRequest::Gltf(path) => self.request_gltf(path),
            AssetRequest::Texture(path, space) => self.request_texture(path, *space),
            AssetRequest::Audio(path) => self.request_audio(path),
        }
    }

    /// Where `request` stands: cached, loading, failed or neither.
    pub fn status(&self, request: &AssetRequest) -> LoadStatus {
        let Ok(path) = Self::normalize(request.path()) else {
            return LoadStatus::Failed;
        };
        let request = request.with_path(path);
        if self.pending.contains_key(&request) {
            return LoadStatus::Loading;
        }
        let loaded = match &request {
            AssetRequest::Gltf(path) => self.scenes.contains_key(path),
            AssetRequest::Texture(path, space) => {
                self.textures.contains_key(&(path.clone(), *space))
            }
            AssetRequest::Audio(path) => self.audio.contains_key(path),
        };
        let failed = self.failed.contains_key(&request)
            || matches!(&request, AssetRequest::Gltf(p) if self.failed_scenes.contains_key(p));
        if loaded {
            LoadStatus::Loaded
        } else if failed {
            LoadStatus::Failed
        } else {
            LoadStatus::NotLoaded
        }
    }

    /// Background requests not registered yet (0 once everything requested is ready).
    pub fn pending_loads(&self) -> usize {
        self.pending.len()
    }

    /// Whether anything at `path` is still loading in the background.
    pub fn is_pending(&self, path: &str) -> bool {
        Self::normalize(path).is_ok_and(|p| self.pending.keys().any(|key| key.path() == p))
    }

    /// Whether a glTF scene, audio stream or sound event at `path` is cached.
    pub fn is_loaded(&self, path: &str) -> bool {
        Self::normalize(path).is_ok_and(|p| {
            self.scenes.contains_key(&p)
                || self.audio.contains_key(&p)
                || self.sound_events.contains_key(&p)
        })
    }
}

impl fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetServer")
            .field("scenes", &self.scenes.len())
            .field("textures", &self.textures.len())
            .field("audio", &self.audio.len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

fn read(source: &dyn AssetSource, path: &str) -> Result<Vec<u8>, AssetError> {
    source.read(path).map_err(|source| AssetError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Keeps a large, long audio file to stream, or decodes it into memory
/// ([`STREAMING_AUDIO_MIN_BYTES`]).
fn decode_audio(path: &str, bytes: Vec<u8>) -> Result<AudioStream, AssetError> {
    let streamed = (bytes.len() >= STREAMING_AUDIO_MIN_BYTES)
        .then(|| {
            let bytes: Arc<[u8]> = Arc::from(bytes.as_slice());
            AudioStream::streaming(bytes)
        })
        // Files the streaming decoder cannot read (no frame count in the header, as in
        // some MP3s) still decode whole.
        .and_then(Result::ok)
        .filter(|s| s.duration().as_secs_f32() >= STREAMING_AUDIO_MIN_SECONDS);
    let stream = match streamed {
        Some(stream) => stream,
        None => {
            AudioStream::from_bytes(bytes).map_err(|e| AssetError::invalid(path, e.to_string()))?
        }
    };
    info!(path, duration = ?stream.duration(), streaming = stream.is_streaming(), "audio loaded");
    Ok(stream)
}
