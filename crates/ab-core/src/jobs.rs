//! Worker threads for the engine's data-parallel work and background tasks.
//!
//! One process-wide pool of worker threads (a work-stealing [`rayon`] pool) is shared by
//! every subsystem: particle simulation, animation sampling, culling and skinning, texture
//! decoding and mipmaps, navigation baking, … The calling thread blocks until a parallel
//! call finishes, so callers keep `&mut` access rules exactly as in serial code; only the
//! loop body runs concurrently.
//!
//! Small inputs stay on the calling thread: below `min_per_job` items (or with a single
//! worker) the helpers run a plain loop, so tiny scenes pay nothing for the pool.
//!
//! Blocking work (file I/O, external processes) does not belong on the pool: it would starve
//! the frame. [`spawn_background`] runs it on its own thread and hands back a [`Task`] to
//! poll from the frame loop or the editor UI.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::OnceLock;
use std::thread;

static WORKERS: OnceLock<usize> = OnceLock::new();

/// Default worker count: every logical core but one, which is left to the main thread, the
/// audio thread and the GPU driver.
pub fn default_worker_count() -> usize {
    thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_sub(1)
        .max(1)
}

/// Starts the worker pool with `threads` workers (`None`: [`default_worker_count`]) and
/// returns the number actually running. Only the first call configures the pool; later
/// calls (a second `Engine`, the editor's play mode) return the existing count.
pub fn init(threads: Option<usize>) -> usize {
    *WORKERS.get_or_init(|| {
        let wanted = threads.unwrap_or_else(default_worker_count).max(1);
        let built = rayon::ThreadPoolBuilder::new()
            .num_threads(wanted)
            .thread_name(|i| format!("ab-worker-{i}"))
            .build_global();
        match built {
            Ok(()) => {
                tracing::debug!(workers = wanted, "job system started");
                wanted
            }
            // Someone (a test harness, a dependency) built the global pool first: use it.
            Err(_) => rayon::current_num_threads(),
        }
    })
}

/// Number of worker threads (starting the pool with defaults if needed).
pub fn worker_count() -> usize {
    init(None)
}

fn serial(len: usize, min_per_job: usize) -> bool {
    len <= min_per_job.max(1) || worker_count() == 1
}

/// Runs `f` on every item, in parallel when there are more than `min_per_job` items.
/// Pick `min_per_job` so one job is worth a few microseconds of work.
pub fn par_for_each_mut<T, F>(items: &mut [T], min_per_job: usize, f: F)
where
    T: Send,
    F: Fn(&mut T) + Sync + Send,
{
    if serial(items.len(), min_per_job) {
        items.iter_mut().for_each(f);
    } else {
        use rayon::prelude::*;
        items
            .par_iter_mut()
            .with_min_len(min_per_job.max(1))
            .for_each(f);
    }
}

/// Runs `f` on every item with its index, in parallel when there are more than
/// `min_per_job` items.
pub fn par_for_each_indexed_mut<T, F>(items: &mut [T], min_per_job: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut T) + Sync + Send,
{
    if serial(items.len(), min_per_job) {
        items
            .iter_mut()
            .enumerate()
            .for_each(|(i, item)| f(i, item));
    } else {
        use rayon::prelude::*;
        items
            .par_iter_mut()
            .with_min_len(min_per_job.max(1))
            .enumerate()
            .for_each(|(i, item)| f(i, item));
    }
}

/// Maps every item, in parallel when there are more than `min_per_job` items. The results
/// keep the input order.
pub fn par_map<T, R, F>(items: &[T], min_per_job: usize, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    if serial(items.len(), min_per_job) {
        items.iter().map(f).collect()
    } else {
        use rayon::prelude::*;
        items
            .par_iter()
            .with_min_len(min_per_job.max(1))
            .map(f)
            .collect()
    }
}

/// Runs `a` and `b`, potentially in parallel, and returns both results.
pub fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    if worker_count() == 1 {
        (a(), b())
    } else {
        rayon::join(a, b)
    }
}

/// The result of work running on another thread, see [`spawn_background`].
#[derive(Debug)]
pub struct Task<T> {
    receiver: Receiver<thread::Result<T>>,
    result: Option<thread::Result<T>>,
    /// The result arrived (it may have been taken since).
    finished: bool,
}

impl<T> Task<T> {
    fn fetch(&mut self) {
        if self.finished {
            return;
        }
        self.result = match self.receiver.try_recv() {
            Ok(result) => Some(result),
            // The thread is gone without sending (it failed to start): report it as a panic
            // rather than wait forever.
            Err(TryRecvError::Disconnected) => Some(Err(Box::new("background task vanished"))),
            Err(TryRecvError::Empty) => return,
        };
        self.finished = true;
    }

    /// The work has finished (successfully or by panicking).
    pub fn is_finished(&mut self) -> bool {
        self.fetch();
        self.finished
    }

    /// Takes the result once the work has finished; `None` while it is still running (and
    /// after the result was taken).
    /// Re-raises a panic of the work on this thread.
    pub fn try_take(&mut self) -> Option<T> {
        self.fetch();
        match self.result.take()? {
            Ok(value) => Some(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Blocks until the work has finished and returns its result.
    pub fn wait(mut self) -> T {
        let result = match self.result.take() {
            Some(result) => result,
            None => self
                .receiver
                .recv()
                .unwrap_or_else(|_| Err(Box::new("background task vanished"))),
        };
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
}

/// Runs `work` on a new named thread, for blocking jobs (disk scans, external processes)
/// that must not hold up a worker. Poll the returned [`Task`] for the result.
pub fn spawn_background<T, F>(name: &str, work: F) -> Task<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (send, receiver) = mpsc::channel();
    let spawned = thread::Builder::new().name(name.to_owned()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        let _ = send.send(result);
    });
    if let Err(err) = spawned {
        tracing::error!("cannot start background thread `{name}`: {err}");
    }
    Task {
        receiver,
        result: None,
        finished: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn pool_has_workers() {
        assert!(worker_count() >= 1);
        assert_eq!(init(Some(64)), worker_count(), "later calls keep the pool");
    }

    #[test]
    fn for_each_touches_every_item_once() {
        let mut items: Vec<u64> = (0..10_000).collect();
        par_for_each_mut(&mut items, 64, |v| *v *= 2);
        assert!(items.iter().enumerate().all(|(i, v)| *v == i as u64 * 2));

        let mut small = vec![1, 2, 3];
        par_for_each_mut(&mut small, 64, |v| *v += 1);
        assert_eq!(small, [2, 3, 4], "small inputs run serially");
    }

    #[test]
    fn indexed_for_each_passes_indices() {
        let mut items = vec![0usize; 5_000];
        par_for_each_indexed_mut(&mut items, 16, |i, v| *v = i);
        assert!(items.iter().enumerate().all(|(i, v)| i == *v));
    }

    #[test]
    fn map_keeps_order() {
        let items: Vec<u32> = (0..5_000).collect();
        let squares = par_map(&items, 32, |v| v * v);
        assert_eq!(squares.len(), items.len());
        assert!(squares.iter().zip(&items).all(|(s, v)| *s == v * v));
    }

    #[test]
    fn work_is_spread_over_threads() {
        if worker_count() < 2 {
            return;
        }
        let threads = std::sync::Mutex::new(std::collections::HashSet::new());
        let calls = AtomicUsize::new(0);
        let mut items = vec![0u8; 256];
        par_for_each_mut(&mut items, 1, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            threads.lock().unwrap().insert(thread::current().id());
            thread::sleep(Duration::from_millis(1));
        });
        assert_eq!(calls.into_inner(), 256);
        assert!(threads.into_inner().unwrap().len() > 1);
    }

    #[test]
    fn join_returns_both() {
        assert_eq!(join(|| 1 + 1, || "b"), (2, "b"));
    }

    #[test]
    fn background_task_reports_its_result() {
        let mut task = spawn_background("test-task", || {
            thread::sleep(Duration::from_millis(5));
            42
        });
        let start = std::time::Instant::now();
        while !task.is_finished() {
            assert!(start.elapsed() < Duration::from_secs(5), "task hung");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(task.try_take(), Some(42));
        assert_eq!(task.try_take(), None, "taken once");
        assert_eq!(spawn_background("test-wait", || "done").wait(), "done");
    }

    #[test]
    fn background_panics_reach_the_caller() {
        let task = spawn_background("test-panic", || -> u32 { panic!("boom") });
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.wait()));
        assert!(caught.is_err());
    }
}
