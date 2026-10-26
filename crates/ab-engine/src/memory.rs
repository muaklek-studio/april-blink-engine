//! Memory profiling: what the engine holds, and how it changes over time.
//!
//! [`MemoryProfiler`] (a resource) samples twice a second:
//!
//! * the heap, when the program installs [`ab_core::memory::CountingAllocator`] (the editor
//!   and the player do): bytes allocated now and at most, allocations and frees;
//! * the engine's assets: meshes, textures, shaders, animations, sounds, loaded models,
//!   and the scene's nodes and entities;
//! * the GPU, as the renderer reports it ([`GpuMemoryReport`]).
//!
//! It keeps a few minutes of samples for graphs, and flags what keeps growing — memory
//! that only goes up while the game runs is how leaks show ([`MemoryProfiler::warnings`]).
//! A baseline can be taken and compared against to see what a level or an action added.
//! Sampling reads counters and sums sizes the engine already has, so it stays cheap.

use ab_core::memory::{self, AllocStats};
use ab_rendering::{GpuMemory, RenderAssets};
use ab_scene::SceneTree;
use std::collections::VecDeque;

/// Seconds between samples.
pub const SAMPLE_EVERY: f64 = 0.5;
/// Samples kept (five minutes).
pub const HISTORY: usize = 600;
/// Samples a growth is judged over (twenty seconds).
const WINDOW: usize = 40;

/// What the renderer holds on the GPU (set by whoever renders the scene).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuMemoryReport(pub GpuMemory);

/// One kind of thing the engine holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Meshes,
    Textures,
    Shaders,
    Animations,
    Sounds,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Meshes,
        Category::Textures,
        Category::Shaders,
        Category::Animations,
        Category::Sounds,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Meshes => "Meshes",
            Category::Textures => "Textures",
            Category::Shaders => "Shaders",
            Category::Animations => "Animations",
            Category::Sounds => "Sounds",
        }
    }
}

/// Memory at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemorySample {
    /// Seconds since sampling started.
    pub time: f64,
    pub heap: AllocStats,
    /// Bytes and counts per [`Category`], in its order.
    pub assets: [(u64, usize); 5],
    pub models: usize,
    pub nodes: usize,
    pub entities: usize,
    pub gpu: GpuMemory,
}

impl MemorySample {
    pub fn asset_bytes(&self) -> u64 {
        self.assets.iter().map(|a| a.0).sum()
    }

    pub fn category(&self, category: Category) -> (u64, usize) {
        self.assets[Category::ALL
            .iter()
            .position(|&c| c == category)
            .expect("listed")]
    }

    /// What the engine holds now.
    pub fn take(tree: &SceneTree, time: f64) -> Self {
        let world = tree.world();
        let mut assets = [(0, 0); 5];
        if let Some(render) = world.resource::<RenderAssets>() {
            assets[0] = (render.mesh_bytes() as u64, render.mesh_count());
            assets[1] = (render.texture_bytes() as u64, render.texture_count());
            assets[2] = (render.shader_bytes() as u64, render.shader_count());
        }
        if let Some(library) = world.resource::<ab_animation::AnimationLibrary>() {
            assets[3] = (library.memory_bytes() as u64, library.len());
        }
        let mut models = 0;
        if let Some(server) = world.resource::<ab_asset::AssetServer>() {
            let (count, bytes) = server.audio_memory();
            assets[4] = (bytes as u64, count);
            models = server.loaded_models();
        }
        Self {
            time,
            heap: memory::stats(),
            assets,
            models,
            nodes: tree.node_count(),
            entities: world.entity_count(),
            gpu: world
                .resource::<GpuMemoryReport>()
                .map(|g| g.0)
                .unwrap_or_default(),
        }
    }
}

/// Something that kept growing over the last samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Growth {
    /// What grew: `Heap`, a [`Category`] name, `Nodes`.
    pub what: String,
    /// How much per minute (bytes, or nodes).
    pub per_minute: f64,
    /// It is a count of nodes, not bytes.
    pub nodes: bool,
}

/// Memory over time (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct MemoryProfiler {
    pub history: VecDeque<MemorySample>,
    /// What to compare against ([`MemoryProfiler::take_baseline`]).
    pub baseline: Option<MemorySample>,
    /// Stop sampling (to read the numbers).
    pub paused: bool,
    /// When sampling started: samples are timed by the wall clock, whoever takes them (the
    /// engine while playing, an editor while editing).
    started: Option<std::time::Instant>,
}

impl MemoryProfiler {
    pub fn latest(&self) -> Option<&MemorySample> {
        self.history.back()
    }

    /// Samples now, when it is time.
    pub fn update(&mut self, tree: &SceneTree) {
        let time = self
            .started
            .get_or_insert_with(std::time::Instant::now)
            .elapsed()
            .as_secs_f64();
        self.update_at(tree, time);
    }

    /// [`MemoryProfiler::update`] at `time` seconds.
    pub fn update_at(&mut self, tree: &SceneTree, time: f64) {
        if self.paused
            || self
                .history
                .back()
                .is_some_and(|s| time - s.time < SAMPLE_EVERY)
        {
            return;
        }
        self.push(MemorySample::take(tree, time));
    }

    pub fn push(&mut self, sample: MemorySample) {
        self.history.push_back(sample);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    /// Remembers now as the baseline.
    pub fn take_baseline(&mut self) {
        self.baseline = self.history.back().cloned();
    }

    /// Allocations per second over the last two samples.
    pub fn allocations_per_second(&self) -> f64 {
        let n = self.history.len();
        if n < 2 {
            return 0.0;
        }
        let (a, b) = (&self.history[n - 2], &self.history[n - 1]);
        let dt = (b.time - a.time).max(1e-6);
        b.heap.allocations.saturating_sub(a.heap.allocations) as f64 / dt
    }

    /// What kept growing over the last twenty seconds: going up in most steps, by more than
    /// a little (see the module docs).
    pub fn warnings(&self) -> Vec<Growth> {
        if self.history.len() < WINDOW {
            return Vec::new();
        }
        let window: Vec<&MemorySample> = self
            .history
            .iter()
            .skip(self.history.len() - WINDOW)
            .collect();
        let mut series: Vec<(String, Vec<f64>, f64, bool)> = Vec::new();
        if window[0].heap.installed {
            // At least 1 MB, and 5% more than it was.
            let first = window[0].heap.live as f64;
            series.push((
                "Heap".into(),
                window.iter().map(|s| s.heap.live as f64).collect(),
                (first * 0.05).max(1024.0 * 1024.0),
                false,
            ));
        }
        for (i, category) in Category::ALL.iter().enumerate() {
            series.push((
                category.name().into(),
                window.iter().map(|s| s.assets[i].0 as f64).collect(),
                256.0 * 1024.0,
                false,
            ));
        }
        series.push((
            "Nodes".into(),
            window.iter().map(|s| s.nodes as f64).collect(),
            50.0,
            true,
        ));
        let seconds = (window[WINDOW - 1].time - window[0].time).max(1e-6);
        series
            .into_iter()
            .filter_map(|(what, values, threshold, nodes)| {
                let growth = values[values.len() - 1] - values[0];
                let rising = values.windows(2).filter(|w| w[1] >= w[0]).count();
                let mostly_up = rising as f64 >= (values.len() - 1) as f64 * 0.8;
                (growth > threshold && mostly_up).then(|| Growth {
                    what,
                    per_minute: growth / seconds * 60.0,
                    nodes,
                })
            })
            .collect()
    }
}

/// Samples the [`MemoryProfiler`], when there is one. The engine calls it every frame.
pub fn update(tree: &SceneTree) {
    let Some(mut profiler) = tree.world().resource_mut::<MemoryProfiler>() else {
        return;
    };
    profiler.update(tree);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: f64, live: usize, meshes: u64, nodes: usize) -> MemorySample {
        let mut s = MemorySample {
            time,
            nodes,
            ..Default::default()
        };
        s.heap.installed = true;
        s.heap.live = live;
        s.assets[0] = (meshes, 1);
        s
    }

    #[test]
    fn steady_growth_is_flagged_and_noise_is_not() {
        let mut profiler = MemoryProfiler::default();
        for i in 0..60 {
            // The heap climbs 200 KB a sample; meshes go up and down; nodes are steady.
            let wobble = if i % 2 == 0 { 0 } else { 900_000 };
            profiler.push(sample(
                i as f64 * 0.5,
                50_000_000 + i * 200_000,
                1_000_000 + wobble,
                100,
            ));
        }
        let warnings = profiler.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].what, "Heap");
        // 200 KB per half second: 24 MB a minute.
        assert!((warnings[0].per_minute - 24_000_000.0).abs() < 1.0);
        // Nodes piling up are flagged too.
        let mut profiler = MemoryProfiler::default();
        for i in 0..40 {
            profiler.push(sample(i as f64 * 0.5, 1000, 0, 100 + i * 5));
        }
        assert_eq!(profiler.warnings()[0].what, "Nodes");
        assert!(profiler.warnings()[0].nodes);
    }

    #[test]
    fn samples_are_taken_twice_a_second_and_kept_for_five_minutes() {
        let tree = SceneTree::new();
        let mut profiler = MemoryProfiler::default();
        profiler.update_at(&tree, 0.0);
        profiler.update_at(&tree, 0.2);
        profiler.update_at(&tree, 0.6);
        assert_eq!(profiler.history.len(), 2);
        for i in 0..(HISTORY + 10) {
            profiler.push(sample(i as f64, 0, 0, 0));
        }
        assert_eq!(profiler.history.len(), HISTORY);
        assert!(profiler.latest().unwrap().nodes == 0);
        profiler.take_baseline();
        assert!(profiler.baseline.is_some());
    }
}
