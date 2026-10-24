use crate::error::AssetError;
use crate::gltf_loader::{self, GltfScene};
use crate::image_loader::decode_image;
use crate::source::{normalize_path, AssetSource, FileSystemSource};
use ab_animation::AnimationLibrary;
use ab_audio::AudioStream;
use ab_rendering::{ColorSpace, RenderAssets, SamplerSettings, TextureHandle};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Loads assets through an [`AssetSource`] and caches them by normalised path.
///
/// Stored as an ECS resource. Loaded resources are registered in the world's
/// [`RenderAssets`] / [`AnimationLibrary`], so callers pass those in.
pub struct AssetServer {
    source: Box<dyn AssetSource>,
    scenes: HashMap<String, Arc<GltfScene>>,
    /// glTF files that failed to load, and why: not decoded again on every spawn (each try
    /// would upload its textures again) until the file changes ([`AssetServer::evict`]).
    failed_scenes: HashMap<String, String>,
    textures: HashMap<(String, ColorSpace), TextureHandle>,
    audio: HashMap<String, AudioStream>,
    /// `RenderAssets::id` the cached textures and scenes were registered in, and
    /// `AnimationLibrary::id` of the scenes' clips. Loading into other ones drops the stale
    /// handles instead of returning them.
    render_id: Option<u64>,
    library_id: Option<u64>,
}

impl AssetServer {
    pub fn new(source: impl AssetSource) -> Self {
        Self {
            source: Box::new(source),
            scenes: HashMap::new(),
            textures: HashMap::new(),
            audio: HashMap::new(),
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

    /// Forgets what was loaded from `path` (its file changed): the next load reads it again.
    pub fn evict(&mut self, path: &str) {
        if let Ok(path) = Self::normalize(path) {
            self.scenes.remove(&path);
            self.failed_scenes.remove(&path);
            self.textures.retain(|(p, _), _| *p != path);
            self.audio.remove(&path);
        }
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
        let texture = decode_image(&path, &bytes, color_space, SamplerSettings::default())?;
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
        let stream = AudioStream::from_bytes(bytes)
            .map_err(|e| AssetError::invalid(&path, e.to_string()))?;
        info!(path, duration = ?stream.duration(), "audio loaded");
        self.audio.insert(path, stream.clone());
        Ok(stream)
    }

    /// Whether a glTF scene or audio stream at `path` is cached.
    pub fn is_loaded(&self, path: &str) -> bool {
        Self::normalize(path)
            .is_ok_and(|p| self.scenes.contains_key(&p) || self.audio.contains_key(&p))
    }
}

impl fmt::Debug for AssetServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetServer")
            .field("scenes", &self.scenes.len())
            .field("textures", &self.textures.len())
            .field("audio", &self.audio.len())
            .finish_non_exhaustive()
    }
}
