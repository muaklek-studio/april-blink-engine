//! Background threads for [`AssetServer`](crate::AssetServer) requests.
//!
//! A few dedicated threads (started on the first request) read and decode files, so the
//! frame never waits on the disk; the work they do is plain data (bytes, decoded images,
//! parsed glTF), handed back over a channel for the server to register on the main thread.
//! Dropping the [`Loader`] lets the threads finish their current job and exit.

use crate::error::AssetError;
use crate::gltf_loader::PreparedGltf;
use ab_audio::AudioStream;
use ab_rendering::{ColorSpace, Texture};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Loading threads: reading is I/O bound and decoding already fans out over the job pool,
/// so a couple keep requests flowing without competing with the frame.
const THREADS: usize = 2;

/// An asset to load in the background ([`AssetServer::request`](crate::AssetServer::request))
/// or to ask about ([`AssetServer::status`](crate::AssetServer::status)).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssetRequest {
    /// A `.gltf`/`.glb` model.
    Gltf(String),
    /// A PNG/JPEG image, sampled in this color space.
    Texture(String, ColorSpace),
    /// A WAV/Ogg/MP3/FLAC sound.
    Audio(String),
}

impl AssetRequest {
    pub fn path(&self) -> &str {
        match self {
            AssetRequest::Texture(path, _)
            | AssetRequest::Gltf(path)
            | AssetRequest::Audio(path) => path,
        }
    }

    /// The same request with its path replaced (normalised).
    pub(crate) fn with_path(&self, path: String) -> Self {
        match self {
            AssetRequest::Texture(_, space) => AssetRequest::Texture(path, *space),
            AssetRequest::Gltf(_) => AssetRequest::Gltf(path),
            AssetRequest::Audio(_) => AssetRequest::Audio(path),
        }
    }
}

/// Where an [`AssetRequest`] stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    /// Not cached and not being loaded (never requested, or evicted since).
    NotLoaded,
    /// Loading in the background.
    Loading,
    /// Cached: loading it returns at once.
    Loaded,
    /// Its background load failed (loading it reports why); requesting it tries again.
    Failed,
}

/// The result of a request.
pub(crate) enum Outcome {
    Texture(Result<Texture, AssetError>),
    Gltf(Result<Box<PreparedGltf>, AssetError>),
    Audio(Result<AudioStream, AssetError>),
}

/// A finished request: its key, the id it was sent with, and the outcome (`Err` if the work
/// panicked).
pub(crate) type Done = (AssetRequest, u64, Result<Outcome, String>);

struct Job {
    key: AssetRequest,
    id: u64,
    work: Box<dyn FnOnce() -> Outcome + Send>,
}

pub(crate) struct Loader {
    /// `None` until the first request starts the threads.
    jobs: Option<Sender<Job>>,
    done_sender: Sender<Done>,
    done: Receiver<Done>,
}

impl Loader {
    pub(crate) fn new() -> Self {
        let (done_sender, done) = mpsc::channel();
        Self {
            jobs: None,
            done_sender,
            done,
        }
    }

    /// Runs `work` on a loading thread; its outcome comes back from [`Loader::finished`].
    pub(crate) fn submit(
        &mut self,
        key: AssetRequest,
        id: u64,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        let job = Job {
            key,
            id,
            work: Box::new(work),
        };
        let jobs = self.jobs.get_or_insert_with(|| start(&self.done_sender));
        if let Err(mpsc::SendError(job)) = jobs.send(job) {
            // Every thread failed to start: report the request as failed instead of
            // leaving it pending forever.
            let _ = self.done_sender.send((
                job.key,
                job.id,
                Err("no asset loading thread is running".to_owned()),
            ));
        }
    }

    /// Requests finished since the last call.
    pub(crate) fn finished(&self) -> impl Iterator<Item = Done> + '_ {
        self.done.try_iter()
    }
}

fn start(done: &Sender<Done>) -> Sender<Job> {
    let (jobs, receiver) = mpsc::channel::<Job>();
    let receiver = Arc::new(Mutex::new(receiver));
    for n in 0..THREADS {
        let receiver = receiver.clone();
        let done = done.clone();
        let spawned = thread::Builder::new()
            .name(format!("ab-asset-load-{n}"))
            .spawn(move || loop {
                // The lock is held only while waiting for the next job.
                let job = match receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                let Ok(Job { key, id, work }) = job else {
                    return; // The loader was dropped.
                };
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
                        panic
                            .downcast_ref::<&str>()
                            .map(|s| (*s).to_owned())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "the loader panicked".to_owned())
                    });
                if done.send((key, id, outcome)).is_err() {
                    return;
                }
            });
        if let Err(err) = spawned {
            tracing::error!("cannot start an asset loading thread: {err}");
        }
    }
    jobs
}
