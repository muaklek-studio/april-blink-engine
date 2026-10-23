//! Thumbnails for the asset browser: images (decoded and shrunk off the UI thread, with
//! normal maps recognised), models, scenes and materials (rendered by the [`Previewer`],
//! one per frame so browsing stays smooth).
//!
//! Every thumbnail is kept in memory while the editor runs and on disk in the editor's
//! cache folder, keyed by the file's path, size and modification time: a changed file gets
//! a new one, an unchanged project opens with its thumbnails ready.

use crate::preview::{Previewer, Shape, Subject};
use crate::project::AssetKind;
use ab_engine::core::jobs::{self, Task};
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Thumbnail side, in pixels.
pub const SIZE: u32 = 128;
/// Bumped when thumbnails are drawn differently, so old cached ones are not used.
const VERSION: u32 = 1;
/// Images decoded at the same time.
const DECODERS: usize = 4;

/// What is known about a file's thumbnail.
#[derive(Clone, Default)]
pub struct Thumbnail {
    pub texture: Option<egui::TextureHandle>,
    /// An image that looks like a tangent-space normal map.
    pub normal_map: bool,
    /// Pixel size of an image.
    pub image_size: Option<[u32; 2]>,
    /// Why there is none (the file is not readable, say).
    pub error: Option<String>,
}

impl std::fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbnail")
            .field("ready", &self.texture.is_some())
            .field("normal_map", &self.normal_map)
            .field("image_size", &self.image_size)
            .field("error", &self.error)
            .finish()
    }
}

/// A finished image decode: the thumbnail and what was learnt.
struct Decoded {
    image: Result<(egui::ColorImage, bool, [u32; 2]), String>,
}

/// File identity: changes when the file does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

struct Entry {
    stamp: Option<Stamp>,
    thumbnail: Thumbnail,
    decoding: Option<Task<Decoded>>,
    /// An image waiting for a free decoder.
    to_decode: bool,
    /// Waiting for the previewer.
    queued: bool,
}

pub struct Thumbnails {
    project: PathBuf,
    cache_dir: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    /// Asset paths waiting for a render, oldest request first.
    render_queue: VecDeque<(String, AssetKind)>,
    previewer: Option<Previewer>,
    gpu: Option<ab_engine::rendering::GpuContext>,
}

impl std::fmt::Debug for Thumbnails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbnails")
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// Whether thumbnails of `kind` exist.
pub fn has_thumbnail(kind: AssetKind) -> bool {
    matches!(
        kind,
        AssetKind::Image | AssetKind::Model | AssetKind::Material | AssetKind::Scene
    )
}

impl Thumbnails {
    pub fn new(project: &Path, gpu: Option<ab_engine::rendering::GpuContext>) -> Self {
        let cache_dir = Some(
            crate::settings::EditorSettings::store()
                .dir()
                .join("thumbnails"),
        );
        Self::with_cache(project, gpu, cache_dir)
    }

    fn with_cache(
        project: &Path,
        gpu: Option<ab_engine::rendering::GpuContext>,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            project: project.to_path_buf(),
            cache_dir,
            entries: HashMap::new(),
            render_queue: VecDeque::new(),
            previewer: None,
            gpu,
        }
    }

    /// The thumbnail of `path` (asked for again every frame it is on screen): ready, or
    /// on its way.
    pub fn get(&mut self, ctx: &egui::Context, path: &str, kind: AssetKind) -> Option<&Thumbnail> {
        if !has_thumbnail(kind) {
            return None;
        }
        let file = self.project.join(path);
        let now = stamp(&file);
        let stale = self.entries.get(path).is_some_and(|e| e.stamp != now);
        if stale || !self.entries.contains_key(path) {
            let mut entry = Entry {
                stamp: now,
                thumbnail: Thumbnail::default(),
                decoding: None,
                to_decode: false,
                queued: false,
            };
            if let Some(cached) = self.read_cache(path, now) {
                entry.thumbnail.texture =
                    Some(ctx.load_texture(path, cached.0, Default::default()));
                entry.thumbnail.normal_map = cached.1;
            } else if kind == AssetKind::Image {
                entry.to_decode = true;
            } else {
                entry.queued = true;
                self.render_queue.push_back((path.to_owned(), kind));
            }
            self.entries.insert(path.to_owned(), entry);
        }
        self.entries.get(path).map(|e| &e.thumbnail)
    }

    /// Finishes decodes and renders the next waiting thumbnail. Returns whether work is
    /// left (to keep repainting).
    pub fn update(&mut self, ctx: &egui::Context) -> bool {
        let mut busy = false;
        let mut finished = Vec::new();
        let mut running = self
            .entries
            .values()
            .filter(|e| e.decoding.is_some())
            .count();
        for (path, entry) in &mut self.entries {
            if entry.to_decode && running < DECODERS {
                entry.to_decode = false;
                entry.decoding = Some(decode(self.project.join(path)));
                running += 1;
            }
            busy |= entry.to_decode;
            let Some(task) = &mut entry.decoding else {
                continue;
            };
            if let Some(done) = task.try_take() {
                entry.decoding = None;
                match done.image {
                    Ok((image, normal_map, size)) => {
                        finished.push((path.clone(), entry.stamp, image.clone(), normal_map));
                        entry.thumbnail.texture =
                            Some(ctx.load_texture(path, image, Default::default()));
                        entry.thumbnail.normal_map = normal_map;
                        entry.thumbnail.image_size = Some(size);
                    }
                    Err(err) => entry.thumbnail.error = Some(err),
                }
            } else {
                busy = true;
            }
        }
        for (path, stamp, image, normal_map) in finished {
            self.write_cache(&path, stamp, &image, normal_map);
        }
        if let Some((path, kind)) = self.render_queue.pop_front() {
            busy = true;
            self.render(ctx, &path, kind);
        }
        busy || !self.render_queue.is_empty()
    }

    fn render(&mut self, ctx: &egui::Context, path: &str, kind: AssetKind) {
        let Some(gpu) = self.gpu.clone() else {
            if let Some(entry) = self.entries.get_mut(path) {
                entry.thumbnail.error = Some("no GPU for previews".into());
            }
            return;
        };
        let project = self.project.clone();
        let previewer = self
            .previewer
            .get_or_insert_with(|| Previewer::new(gpu, &project));
        let subject = match kind {
            AssetKind::Model => Subject::Model(path.to_owned()),
            AssetKind::Scene => Subject::Scene(path.to_owned()),
            _ => Subject::Material(path.to_owned(), Shape::Sphere),
        };
        // Read the file again: the thumbnail is made because it is new or changed.
        let result = previewer.thumbnail(&subject, SIZE, true);
        let Some(entry) = self.entries.get_mut(path) else {
            return;
        };
        entry.queued = false;
        match result {
            Ok(image) => {
                let stamp = entry.stamp;
                entry.thumbnail.texture =
                    Some(ctx.load_texture(path, image.clone(), Default::default()));
                self.write_cache(path, stamp, &image, false);
            }
            Err(err) => {
                tracing::debug!("no thumbnail for {path}: {err}");
                entry.thumbnail.error = Some(err);
            }
        }
    }

    fn cache_file(&self, path: &str, stamp: Option<Stamp>) -> Option<PathBuf> {
        let stamp = stamp?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (VERSION, &self.project, path, stamp).hash(&mut hasher);
        Some(
            self.cache_dir
                .as_ref()?
                .join(format!("{:016x}.png", hasher.finish())),
        )
    }

    /// A thumbnail saved by an earlier run. A normal map has an empty `.normal` file next to
    /// its thumbnail.
    fn read_cache(&self, path: &str, stamp: Option<Stamp>) -> Option<(egui::ColorImage, bool)> {
        let file = self.cache_file(path, stamp)?;
        let normal_map = file.with_extension("normal").is_file();
        let image = image::open(&file).ok()?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        Some((
            egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
            normal_map,
        ))
    }

    fn write_cache(
        &self,
        path: &str,
        stamp: Option<Stamp>,
        image: &egui::ColorImage,
        normal_map: bool,
    ) {
        let Some(file) = self.cache_file(path, stamp) else {
            return;
        };
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
        let saved = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .ok()
            .and_then(|()| {
                image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, bytes)
            })
            .map(|img| img.save(&file));
        if let Some(Err(err)) = saved {
            tracing::debug!("thumbnail not cached: {err}");
        }
        if normal_map {
            let _ = std::fs::write(file.with_extension("normal"), b"");
        }
    }
}

/// Decodes and shrinks an image off the UI thread.
fn decode(file: PathBuf) -> Task<Decoded> {
    jobs::spawn_background("ab-thumbnail", move || Decoded {
        image: decode_now(&file),
    })
}

fn decode_now(file: &Path) -> Result<(egui::ColorImage, bool, [u32; 2]), String> {
    let image = image::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let size = [image.width(), image.height()];
    let small = image.thumbnail(SIZE, SIZE).to_rgba8();
    let name = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let normal_map = looks_like_normal_map(&name, small.as_raw());
    let pixels = [small.width() as usize, small.height() as usize];
    Ok((
        egui::ColorImage::from_rgba_unmultiplied(pixels, small.as_raw()),
        normal_map,
        size,
    ))
}

/// A tangent-space normal map by its name (`_n`, `normal`, `nrm`) or its colors: mostly
/// blue-violet (normals pointing out of the surface: x, y about 0.5, z near 1).
pub fn looks_like_normal_map(name: &str, rgba: &[u8]) -> bool {
    let named = name.contains("normal")
        || name.contains("nrm")
        || name.ends_with("_n")
        || name.ends_with("-n");
    let pixels = rgba.len() / 4;
    if pixels == 0 {
        return named;
    }
    let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
    for p in rgba.as_chunks::<4>().0 {
        r += f32::from(p[0]);
        g += f32::from(p[1]);
        b += f32::from(p[2]);
    }
    let n = pixels as f32 * 255.0;
    let (r, g, b) = (r / n, g / n, b / n);
    let colors = b > 0.75 && (r - 0.5).abs() < 0.15 && (g - 0.5).abs() < 0.15;
    named || colors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_maps_are_recognised() {
        let flat_normal = [128u8, 128, 255, 255].repeat(16);
        let brick = [170u8, 70, 50, 255].repeat(16);
        assert!(looks_like_normal_map("wall", &flat_normal));
        assert!(!looks_like_normal_map("wall", &brick));
        assert!(looks_like_normal_map("wall_n", &brick), "by name");
        assert!(looks_like_normal_map("rock_normal", &brick));
    }

    #[test]
    fn images_decode_in_the_background_and_come_back_from_the_cache() {
        let assets = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let cache = std::env::temp_dir().join(format!("ab-thumbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let ctx = egui::Context::default();
        let mut thumbnails = Thumbnails::with_cache(&assets, None, Some(cache.clone()));
        let path = "models/totem_albedo.png";
        assert!(thumbnails
            .get(&ctx, path, AssetKind::Image)
            .unwrap()
            .texture
            .is_none());
        let start = std::time::Instant::now();
        while thumbnails.update(&ctx) {
            assert!(start.elapsed().as_secs() < 10, "decode hung");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first = thumbnails
            .get(&ctx, path, AssetKind::Image)
            .unwrap()
            .clone();
        assert!(first.texture.is_some() && first.image_size.is_some());
        assert!(!first.normal_map);
        let files = std::fs::read_dir(&cache).unwrap().count();
        assert_eq!(files, 1, "one cached thumbnail");

        // A new session: straight from the cache, no decode.
        let mut again = Thumbnails::with_cache(&assets, None, Some(cache.clone()));
        assert!(again
            .get(&ctx, path, AssetKind::Image)
            .unwrap()
            .texture
            .is_some());
        // Kinds without thumbnails, and models without a GPU.
        assert!(again
            .get(&ctx, "audio/thud.wav", AssetKind::Audio)
            .is_none());
        again.get(&ctx, "models/crate.glb", AssetKind::Model);
        again.update(&ctx);
        let model = again
            .get(&ctx, "models/crate.glb", AssetKind::Model)
            .unwrap();
        assert!(model.error.is_some());
        let _ = std::fs::remove_dir_all(&cache);
    }
}
