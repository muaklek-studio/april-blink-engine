use crate::config::EngineConfig;
use crate::debug::{debug_shapes_system, DebugSettings};
use crate::debug_overlay::{self, DebugOverlay};
use crate::hot_reload::HotReload;
use crate::profiler::{FrameSample, Profiler};
use ab_animation::animation_system;
use ab_asset::AssetServer;
use ab_audio::{audio_system, Audio};
use ab_core::{FixedTimestep, Time};
use ab_ecs::{IntoSystem, Schedule, ScheduleStage};
use ab_input::KeyCode;
use ab_input::{Actions, InputState};
use ab_physics::{physics_system, PhysicsWorld};
use ab_platform::PlatformEvent;
use ab_rendering::{debug_draw_system, DebugDraw, Environment, FrameStats, RenderAssets};
use ab_scene::{SceneTree, ScriptProfile, ScriptRegistry};
use ab_ui::{ui_animation_system, ui_input_system};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;
use tracing::error;

/// Engine-level requests that gameplay code can make through the world, e.g.
/// `tree.world().resource_mut::<EngineControl>().unwrap().quit()`.
#[derive(Debug, Clone, Default)]
pub struct EngineControl {
    quit_requested: bool,
    mouse_captured: bool,
    fullscreen: bool,
}

impl EngineControl {
    /// Asks the engine to exit after the current frame.
    pub fn quit(&mut self) {
        self.quit_requested = true;
    }

    pub fn is_quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Hides and locks the cursor for mouse look (applied by the window at end of frame).
    /// Raw mouse motion keeps arriving in `InputState::mouse_delta` / `Actions::look`.
    pub fn set_mouse_captured(&mut self, captured: bool) {
        self.mouse_captured = captured;
    }

    pub fn is_mouse_captured(&self) -> bool {
        self.mouse_captured
    }

    /// Borderless fullscreen or windowed (applied by the window at end of frame).
    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        self.fullscreen = fullscreen;
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }
}

/// Statistics of the last rendered frame (zero when running headless).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats(pub FrameStats);

/// The running engine: scene tree, systems and timing. Independent of any window, so it can
/// be stepped manually in tests and tools via [`Engine::update`].
pub struct Engine {
    tree: SceneTree,
    schedule: Schedule,
    fixed: FixedTimestep,
    started: bool,
    hot_reload: Option<HotReload>,
    debug_overlay_key: Option<KeyCode>,
    debug_physics_key: Option<KeyCode>,
    debug_input_key: Option<KeyCode>,
    debug_trace_key: Option<KeyCode>,
    fullscreen_key: Option<KeyCode>,
}

impl Engine {
    /// Creates the engine and registers the core resources: [`Time`], [`InputState`],
    /// [`Actions`], [`EngineControl`], [`RenderAssets`], [`Environment`], [`RenderStats`],
    /// [`AssetServer`], [`AnimationLibrary`], [`PhysicsWorld`], [`Audio`] and the UI
    /// resources (see [`ab_ui::register`]), plus the built-in systems (action evaluation
    /// and UI input in `PreUpdate`, physics in `PostFixedUpdate`, animation and UI
    /// animation in `Update`, audio in `PostUpdate`). Starts the worker threads
    /// ([`ab_core::jobs`]) on first use.
    pub fn new(config: &EngineConfig) -> Self {
        ab_core::jobs::init(config.worker_threads);
        let fixed = FixedTimestep::new(config.physics_hz, config.max_physics_steps_per_frame);
        let mut tree = SceneTree::new();
        let world = tree.world_mut();
        world.insert_resource(Time::new(fixed.step()));
        world.insert_resource(InputState::new());
        world.insert_resource(Actions::new(config.input_map.clone()));
        world.insert_resource(EngineControl::default());
        world.insert_resource(RenderAssets::new());
        world.insert_resource(Environment::default());
        world.insert_resource(RenderStats::default());
        let executable = std::env::current_exe().ok();
        world.insert_resource(AssetServer::new(crate::assets::mount_assets(
            &config.asset_root,
            executable.as_deref(),
        )));
        ab_animation::register(world);
        world.insert_resource(PhysicsWorld::new());
        world.insert_resource(Audio::new(config.audio_output));
        world.insert_resource(Profiler::new());
        world.insert_resource(DebugOverlay::default());
        world.insert_resource(crate::input_overlay::InputOverlay::default());
        world.insert_resource(ab_input::InputLog::default());
        world.insert_resource(crate::trace::Tracer::default());
        world.insert_resource(crate::trace::TraceOverlay::default());
        world.insert_resource(DebugDraw::default());
        world.insert_resource(DebugSettings::default());
        world.insert_resource(crate::tween::Tweens::default());
        ab_ui::register(world);
        ab_effects::register(world);
        let mut schedule = Schedule::new();
        schedule.add_system(
            ScheduleStage::PreUpdate,
            ab_ecs::named("debug_draw", debug_draw_system),
        );
        schedule.add_system(
            ScheduleStage::PreUpdate,
            ab_ecs::named("actions", update_actions),
        );
        schedule.add_system(
            ScheduleStage::PreUpdate,
            ab_ecs::named("input_log", crate::input_overlay::record_input_log),
        );
        schedule.add_system(
            ScheduleStage::PreUpdate,
            ab_ecs::named("ui_input", ui_input_system),
        );
        schedule.add_system(
            ScheduleStage::PostUpdate,
            ab_ecs::named("localize_ui", crate::localization::localize_ui_system),
        );
        schedule.add_system(
            ScheduleStage::PostFixedUpdate,
            ab_ecs::named("physics", physics_system),
        );
        schedule.add_system(
            ScheduleStage::Update,
            ab_ecs::named("locomotion", ab_animation::locomotion_system),
        );
        schedule.add_system(
            ScheduleStage::Update,
            ab_ecs::named("animation", animation_system),
        );
        schedule.add_system(
            ScheduleStage::Update,
            ab_ecs::named("head_bob", crate::head_bob::head_bob_system),
        );
        schedule.add_system(
            ScheduleStage::Update,
            ab_ecs::named("ui_animation", ui_animation_system),
        );
        schedule.add_system(
            ScheduleStage::Update,
            ab_ecs::named("tweens", crate::tween::tween_system),
        );
        schedule.add_system(
            ScheduleStage::PostUpdate,
            ab_ecs::named("audio", audio_system),
        );
        schedule.add_system(
            ScheduleStage::PostUpdate,
            ab_ecs::named("effects", ab_effects::effects_system),
        );
        schedule.add_system(
            ScheduleStage::PostUpdate,
            ab_ecs::named("blood", crate::blood::blood_system),
        );
        schedule.add_system(
            ScheduleStage::PostUpdate,
            ab_ecs::named("debug_shapes", debug_shapes_system),
        );
        Self {
            tree,
            schedule,
            fixed,
            started: false,
            hot_reload: None,
            debug_overlay_key: config.debug_overlay_key,
            debug_physics_key: config.debug_physics_key,
            debug_input_key: config.debug_input_key,
            debug_trace_key: config.debug_trace_key,
            fullscreen_key: config.fullscreen_key,
        }
    }

    pub fn tree(&self) -> &SceneTree {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut SceneTree {
        &mut self.tree
    }

    /// Registers an engine system (renderer, physics, audio, ...) in `stage`.
    pub fn add_system(&mut self, stage: ScheduleStage, system: impl IntoSystem) -> &mut Self {
        self.schedule.add_system(stage, system);
        self
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// Snapshot of the frame timing.
    pub fn time(&self) -> Time {
        self.tree
            .world()
            .resource::<Time>()
            .expect("Time resource is registered")
            .clone()
    }

    pub fn is_quit_requested(&self) -> bool {
        self.tree
            .world()
            .resource::<EngineControl>()
            .is_some_and(|control| control.is_quit_requested())
    }

    pub fn request_quit(&mut self) {
        if let Some(mut control) = self.tree.world().resource_mut::<EngineControl>() {
            control.quit();
        }
    }

    /// Feeds a platform event to the engine.
    pub fn handle_event(&mut self, event: &PlatformEvent) {
        if let PlatformEvent::CloseRequested = event {
            self.request_quit();
        }
        if let Some(mut input) = self.tree.world().resource_mut::<InputState>() {
            input.handle_event(event);
        }
    }

    /// Watches `library` (a script library built with the `ab-script` SDK as a `cdylib`) and
    /// swaps scripts attached by name for its versions whenever it is rebuilt. Loads it right
    /// away; on error the engine keeps watching, so fixing the build retries.
    pub fn enable_hot_reload(&mut self, library: impl Into<PathBuf>) -> anyhow::Result<usize> {
        let hot = self.hot_reload.insert(HotReload::new(library.into()));
        hot.load(&mut self.tree)
    }

    /// The library being watched for hot reload, if any.
    pub fn hot_reload_library(&self) -> Option<&Path> {
        self.hot_reload.as_ref().map(|h| h.library())
    }

    /// For editors: when the hot-reload library was rebuilt, loads it and returns the scripts
    /// it registers, without swapping any script in the tree (scenes being edited do not run
    /// scripts). `None` when nothing changed; the next [`Engine::update`] will not load the
    /// same build again.
    pub fn poll_script_library(&mut self) -> Option<anyhow::Result<ScriptRegistry>> {
        let hot = self.hot_reload.as_mut()?;
        hot.poll().then(|| hot.load_registry())
    }

    /// Advances one frame by `raw_delta` seconds of wall-clock time.
    pub fn update(&mut self, raw_delta: f32) {
        if let Some(hot) = &mut self.hot_reload {
            if hot.poll() {
                if let Err(err) = hot.load(&mut self.tree) {
                    error!("hot reload failed: {err:#}");
                }
            }
        }
        let frame_start = Instant::now();
        self.handle_debug_keys();
        self.schedule.reset_timings();
        if !self.started {
            self.started = true;
            self.schedule
                .run_stage(ScheduleStage::Startup, self.tree.world_mut());
        }

        let delta = self.with_time(|time| time.begin_frame(raw_delta));
        let mut sections = [Duration::ZERO; 6];
        let mut timed = |slot: usize, start: Instant| sections[slot] += start.elapsed();

        let start = Instant::now();
        self.schedule
            .run_stage(ScheduleStage::PreUpdate, self.tree.world_mut());
        timed(0, start);

        let fixed_delta = self.fixed.step();
        let fixed_start = Instant::now();
        for _ in 0..self.fixed.advance(delta) {
            self.schedule
                .run_stage(ScheduleStage::FixedUpdate, self.tree.world_mut());
            let start = Instant::now();
            self.tree.physics_process(fixed_delta);
            timed(1, start);
            self.schedule
                .run_stage(ScheduleStage::PostFixedUpdate, self.tree.world_mut());
            self.with_time(|time| time.physics_frame_count += 1);
        }
        timed(2, fixed_start);

        let start = Instant::now();
        self.schedule
            .run_stage(ScheduleStage::Update, self.tree.world_mut());
        timed(3, start);
        let start = Instant::now();
        self.tree.process(delta);
        // Before PostUpdate: its hit lines join this frame's debug drawing.
        crate::trace::update(&mut self.tree, delta);
        timed(4, start);

        let start = Instant::now();
        for stage in [
            ScheduleStage::PostUpdate,
            ScheduleStage::Render,
            ScheduleStage::Cleanup,
        ] {
            self.schedule.run_stage(stage, self.tree.world_mut());
        }
        timed(5, start);

        self.update_debug_overlay(delta);
        if let Some(mut input) = self.tree.world().resource_mut::<InputState>() {
            input.end_frame();
        }
        self.with_time(|time| time.end_frame());
        self.record_profile(&sections, frame_start.elapsed(), delta);
    }

    fn update_debug_overlay(&mut self, delta: f32) {
        debug_overlay::update(&mut self.tree, delta);
        crate::input_overlay::update(&mut self.tree);
    }

    /// Debug keys act at the start of the frame, so their views show in the same frame.
    fn handle_debug_keys(&mut self) {
        let toggle = self.debug_overlay_key.is_some_and(|key| {
            self.tree
                .world()
                .resource::<InputState>()
                .is_some_and(|input| input.is_key_just_pressed(key))
        });
        if toggle {
            if let Some(mut overlay) = self.tree.world().resource_mut::<DebugOverlay>() {
                overlay.toggle();
            }
        }
        let fullscreen_toggle = self.fullscreen_key.is_some_and(|key| {
            self.tree
                .world()
                .resource::<InputState>()
                .is_some_and(|input| input.is_key_just_pressed(key))
        });
        if fullscreen_toggle {
            if let Some(mut control) = self.tree.world().resource_mut::<EngineControl>() {
                let fullscreen = control.is_fullscreen();
                control.set_fullscreen(!fullscreen);
            }
        }
        let physics_toggle = self.debug_physics_key.is_some_and(|key| {
            self.tree
                .world()
                .resource::<InputState>()
                .is_some_and(|input| input.is_key_just_pressed(key))
        });
        let input_toggle = self.debug_input_key.is_some_and(|key| {
            self.tree
                .world()
                .resource::<InputState>()
                .is_some_and(|input| input.is_key_just_pressed(key))
        });
        let trace_toggle = self.debug_trace_key.is_some_and(|key| {
            self.tree
                .world()
                .resource::<InputState>()
                .is_some_and(|input| input.is_key_just_pressed(key))
        });
        if trace_toggle {
            if let Some(mut overlay) = self
                .tree
                .world()
                .resource_mut::<crate::trace::TraceOverlay>()
            {
                overlay.toggle();
            }
        }
        if input_toggle {
            if let Some(mut overlay) = self
                .tree
                .world()
                .resource_mut::<crate::input_overlay::InputOverlay>()
            {
                overlay.toggle();
            }
        }
        if physics_toggle {
            if let Some(mut settings) = self.tree.world().resource_mut::<DebugSettings>() {
                settings.physics = !settings.physics;
            }
        }
    }

    fn record_profile(&mut self, sections: &[Duration; 6], frame: Duration, delta: f32) {
        let scripts = self
            .tree
            .world()
            .resource_mut::<ScriptProfile>()
            .map(|mut p| p.take())
            .unwrap_or_default();
        let Some(mut profiler) = self.tree.world().resource_mut::<Profiler>() else {
            return;
        };
        let unscaled = self
            .tree
            .world()
            .resource::<Time>()
            .map_or(delta, |t| t.unscaled_delta);
        profiler.push_frame(unscaled);
        let mut name = String::new();
        for timing in scripts {
            name.clear();
            name.push_str("script.");
            name.push_str(timing.name);
            profiler.record(&name, timing.total);
        }
        profiler.record("frame", frame);
        for (name, duration) in [
            "pre_update",
            "scripts.physics_process",
            "fixed",
            "update",
            "scripts.process",
            "post_update",
        ]
        .into_iter()
        .zip(sections)
        {
            profiler.record(name, *duration);
        }
        let mut name = String::new();
        for stage in ScheduleStage::ALL {
            for (system, duration) in self.schedule.system_timings(stage) {
                name.clear();
                name.push_str("system.");
                name.push_str(system);
                profiler.record(&name, duration);
            }
        }
        let ms = |d: &Duration| d.as_secs_f32() * 1000.0;
        let sample = FrameSample {
            delta_ms: unscaled * 1000.0,
            update_ms: ms(&frame),
            physics_ms: profiler.last_ms("system.physics").unwrap_or(0.0),
            scripts_ms: ms(&sections[1]) + ms(&sections[4]),
            render_ms: profiler.last_ms("render").unwrap_or(0.0),
        };
        profiler.push_sample(sample);
    }

    fn with_time<R>(&mut self, f: impl FnOnce(&mut Time) -> R) -> R {
        f(&mut self
            .tree
            .world()
            .resource_mut::<Time>()
            .expect("Time resource is registered"))
    }
}

/// Evaluates the action map against this frame's input.
fn update_actions(world: &mut ab_ecs::World) {
    let (Some(input), Some(mut actions)) = (
        world.resource::<InputState>(),
        world.resource_mut::<Actions>(),
    ) else {
        return;
    };
    actions.update(&input);
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("tree", &self.tree)
            .field("schedule", &self.schedule)
            .field("fixed_step", &self.fixed.step())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_ecs::World;
    use ab_input::KeyCode;
    use ab_scene::{NodeHandle, NodeScript};

    #[derive(Default)]
    struct Counters {
        startup: u32,
        fixed: u32,
        update: u32,
        render: u32,
    }

    fn engine_with_counters() -> Engine {
        let mut engine = Engine::new(&EngineConfig::default());
        engine
            .tree_mut()
            .world_mut()
            .insert_resource(Counters::default());
        engine
            .add_system(ScheduleStage::Startup, |w: &mut World| {
                w.resource_mut::<Counters>().unwrap().startup += 1
            })
            .add_system(ScheduleStage::FixedUpdate, |w: &mut World| {
                w.resource_mut::<Counters>().unwrap().fixed += 1
            })
            .add_system(ScheduleStage::Update, |w: &mut World| {
                w.resource_mut::<Counters>().unwrap().update += 1
            })
            .add_system(ScheduleStage::Render, |w: &mut World| {
                w.resource_mut::<Counters>().unwrap().render += 1
            });
        engine
    }

    #[test]
    fn one_second_runs_sixty_fixed_steps() {
        let mut engine = engine_with_counters();
        for _ in 0..120 {
            engine.update(1.0 / 120.0);
        }
        let world = engine.tree().world();
        let counters = world.resource::<Counters>().unwrap();
        assert_eq!(counters.startup, 1);
        assert_eq!(counters.update, 120);
        assert_eq!(counters.render, 120);
        assert!(
            (59..=60).contains(&counters.fixed),
            "fixed steps: {}",
            counters.fixed
        );
        let time = engine.time();
        assert_eq!(time.frame_count, 120);
        assert_eq!(time.physics_frame_count, counters.fixed as u64);
        assert!((time.elapsed - 1.0).abs() < 1e-4);
    }

    #[test]
    fn scripts_receive_both_deltas() {
        struct Mover;
        impl NodeScript for Mover {
            fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
                tree.translate(node, ab_core::Vec3::X * delta).unwrap();
            }
            fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
                tree.translate(node, ab_core::Vec3::Y * delta).unwrap();
            }
        }

        let mut engine = Engine::new(&EngineConfig::default());
        let tree = engine.tree_mut();
        let node = tree.create_node3d("Mover");
        tree.set_script(node, Mover).unwrap();
        tree.add_child(tree.root(), node).unwrap();

        for _ in 0..30 {
            engine.update(1.0 / 30.0);
        }
        let position = engine.tree().position(node).unwrap();
        assert!((position.x - 1.0).abs() < 1e-3, "{position}");
        assert!((position.y - 1.0).abs() < 0.02, "{position}");
    }

    #[test]
    fn input_reaches_scripts_and_clears_each_frame() {
        struct QuitOnEscape;
        impl NodeScript for QuitOnEscape {
            fn process(&mut self, tree: &mut SceneTree, _: NodeHandle, _: f32) {
                let world = tree.world();
                if world
                    .resource::<InputState>()
                    .unwrap()
                    .is_key_just_pressed(KeyCode::Escape)
                {
                    world.resource_mut::<EngineControl>().unwrap().quit();
                }
            }
        }

        let mut engine = Engine::new(&EngineConfig::default());
        let tree = engine.tree_mut();
        let node = tree.create_node("Quitter");
        tree.set_script(node, QuitOnEscape).unwrap();
        tree.add_child(tree.root(), node).unwrap();

        engine.update(0.016);
        assert!(!engine.is_quit_requested());
        engine.handle_event(&PlatformEvent::Key {
            code: KeyCode::Escape,
            pressed: true,
            repeat: false,
        });
        engine.update(0.016);
        assert!(engine.is_quit_requested());
        let world = engine.tree().world();
        assert!(!world
            .resource::<InputState>()
            .unwrap()
            .is_key_just_pressed(KeyCode::Escape));
    }

    #[test]
    fn imported_animations_play_during_update() {
        let config = EngineConfig {
            asset_root: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        let tree = engine.tree_mut();
        let totem = ab_asset::spawn_gltf(tree, "models/totem.gltf").unwrap();
        tree.add_child(tree.root(), totem).unwrap();
        let player = tree.get_node(totem, "AnimationPlayer").unwrap();
        let joint = tree.get_node(totem, "Totem/Joint0").unwrap();
        assert!(tree
            .component_mut::<ab_animation::AnimationPlayer>(player)
            .unwrap()
            .play_named("Spin"));

        // "Spin" turns Joint0 a quarter turn per second.
        for _ in 0..30 {
            engine.update(1.0 / 60.0);
        }
        let rotation = engine.tree().rotation(joint).unwrap();
        let expected = ab_core::Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(rotation.abs_diff_eq(expected, 1e-3), "{rotation}");
    }

    #[test]
    fn actions_drive_scripts() {
        struct Walker;
        impl NodeScript for Walker {
            fn process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
                let direction = {
                    let actions = tree.world().resource::<Actions>().unwrap();
                    actions.axis("move_back", "move_forward")
                };
                tree.translate(node, ab_core::Vec3::NEG_Z * direction * delta)
                    .unwrap();
            }
        }

        let mut map = ab_input::InputMap::new();
        map.bind("move_forward", ab_input::Binding::Key(KeyCode::KeyW))
            .bind("move_back", ab_input::Binding::Key(KeyCode::KeyS));
        let config = EngineConfig {
            input_map: map,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        let tree = engine.tree_mut();
        let walker = tree.create_node3d("Walker");
        tree.set_script(walker, Walker).unwrap();
        tree.add_child(tree.root(), walker).unwrap();

        engine.handle_event(&PlatformEvent::Key {
            code: KeyCode::KeyW,
            pressed: true,
            repeat: false,
        });
        for _ in 0..10 {
            engine.update(0.1);
        }
        let z = engine.tree().position(walker).unwrap().z;
        assert!((z + 1.0).abs() < 1e-4, "walked 1 m forward, z = {z}");
    }

    #[test]
    fn physics_runs_after_physics_process() {
        use ab_physics::{CharacterBody3D, PhysicsBody3D, PhysicsNodes, Shape};

        /// Walks +X under gravity, like a Godot `CharacterBody3D` script.
        struct Walker;
        impl NodeScript for Walker {
            fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, delta: f32) {
                if let Some(mut body) = tree.component_mut::<CharacterBody3D>(node) {
                    body.velocity += ab_physics::PhysicsWorld::DEFAULT_GRAVITY * delta;
                    body.velocity.x = 1.0;
                }
                tree.move_and_slide(node, delta).unwrap();
            }
        }

        let mut engine = Engine::new(&EngineConfig::default());
        let tree = engine.tree_mut();
        let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
        tree.add_collision_shape(floor, Shape::cuboid(ab_core::Vec3::new(20.0, 1.0, 20.0)))
            .unwrap();
        tree.set_position(floor, ab_core::Vec3::new(0.0, -0.5, 0.0))
            .unwrap();
        tree.add_child(tree.root(), floor).unwrap();
        let crate_body = tree.create_physics_body3d("Crate", PhysicsBody3D::rigid());
        tree.add_collision_shape(crate_body, Shape::cuboid(ab_core::Vec3::ONE))
            .unwrap();
        tree.set_position(crate_body, ab_core::Vec3::new(5.0, 3.0, 0.0))
            .unwrap();
        tree.add_child(tree.root(), crate_body).unwrap();
        let player = tree.create_character_body3d("Player", CharacterBody3D::default());
        tree.add_collision_shape(player, Shape::capsule(0.4, 1.8))
            .unwrap();
        tree.set_position(player, ab_core::Vec3::new(0.0, 2.0, 0.0))
            .unwrap();
        tree.set_script(player, Walker).unwrap();
        tree.add_child(tree.root(), player).unwrap();

        for _ in 0..120 {
            engine.update(1.0 / 60.0);
        }
        let tree = engine.tree();
        let crate_position = tree.position(crate_body).unwrap();
        assert!((crate_position.y - 0.5).abs() < 0.02, "{crate_position}");
        let player_position = tree.position(player).unwrap();
        assert!((player_position.y - 0.9).abs() < 0.05, "{player_position}");
        assert!((player_position.x - 2.0).abs() < 0.05, "{player_position}");
        assert!(tree
            .component::<CharacterBody3D>(player)
            .unwrap()
            .is_on_floor());
        // Global transforms are propagated for the renderer.
        let global = tree.global_position(crate_body).unwrap();
        assert!(global.abs_diff_eq(crate_position, 1e-6));
    }

    #[test]
    fn sound_plays_on_a_physics_event() {
        use ab_audio::{AudioNodes, AudioPlayer, AudioStream};
        use ab_physics::{PhysicsBody3D, PhysicsEvent, PhysicsNodes, PhysicsWorld, Shape};

        /// Plays the "Thud" child when its body starts touching something.
        struct ThudOnImpact;
        impl NodeScript for ThudOnImpact {
            fn physics_process(&mut self, tree: &mut SceneTree, node: NodeHandle, _: f32) {
                let hit = tree
                    .world()
                    .resource::<PhysicsWorld>()
                    .unwrap()
                    .events()
                    .iter()
                    .any(|e| matches!(e, PhysicsEvent::ContactStarted { .. }) && e.involves(node));
                if hit {
                    let thud = tree.get_node(node, "Thud").unwrap();
                    tree.component_mut::<AudioPlayer>(thud).unwrap().play();
                }
            }
        }

        let config = EngineConfig {
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        let tree = engine.tree_mut();
        let floor = tree.create_physics_body3d("Floor", PhysicsBody3D::fixed());
        tree.add_collision_shape(floor, Shape::cuboid(ab_core::Vec3::new(10.0, 1.0, 10.0)))
            .unwrap();
        tree.set_position(floor, ab_core::Vec3::new(0.0, -0.5, 0.0))
            .unwrap();
        tree.add_child(tree.root(), floor).unwrap();
        let ball = tree.create_physics_body3d("Ball", PhysicsBody3D::rigid());
        tree.add_collision_shape(ball, Shape::sphere(0.5)).unwrap();
        tree.set_position(ball, ab_core::Vec3::new(0.0, 1.5, 0.0))
            .unwrap();
        // 0.2 s of silence is enough to observe playback.
        let thud = AudioStream::from_samples(8000, &[0.0; 1600]);
        let player = tree.create_audio_stream_player3d("Thud", AudioPlayer::new(thud));
        tree.add_child(ball, player).unwrap();
        tree.set_script(ball, ThudOnImpact).unwrap();
        tree.add_child(tree.root(), ball).unwrap();

        let playing = |engine: &Engine| {
            engine
                .tree()
                .component::<AudioPlayer>(player)
                .unwrap()
                .is_playing()
        };
        // Falling 1 m takes about 0.45 s.
        for _ in 0..20 {
            engine.update(1.0 / 60.0);
        }
        assert!(!playing(&engine), "silent while falling");
        let mut landed_at = None;
        for frame in 20..60 {
            engine.update(1.0 / 60.0);
            if playing(&engine) {
                landed_at = Some(frame);
                break;
            }
        }
        let landed_at = landed_at.expect("the impact plays the sound");
        assert!((24..32).contains(&landed_at), "landed at frame {landed_at}");

        let mut finished = false;
        for _ in 0..30 {
            engine.update(1.0 / 60.0);
            let world = engine.tree().world();
            finished |= world
                .resource::<Audio>()
                .unwrap()
                .finished()
                .contains(&player);
        }
        assert!(finished && !playing(&engine), "the 0.2 s sound ended");
    }

    #[test]
    fn f8_shows_the_input_overlay_and_held_actions() {
        let config = EngineConfig {
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        if let Some(mut actions) = engine.tree().world().resource_mut::<Actions>() {
            actions
                .map_mut()
                .bind("jump", ab_input::Binding::Key(KeyCode::Space));
            actions.hold("jump", 1.0);
        }
        let key = |code, pressed| PlatformEvent::Key {
            code,
            pressed,
            repeat: false,
        };
        engine.handle_event(&key(KeyCode::F8, true));
        engine.update(1.0 / 60.0);
        engine.handle_event(&key(KeyCode::F8, false));
        let tree = engine.tree();
        let overlay = tree
            .find_child(tree.root(), "InputOverlay", false)
            .expect("F8 shows it");
        let label = tree.find_child(overlay, "Text", true).unwrap();
        let text = tree.component::<ab_ui::Label>(label).unwrap().text.clone();
        assert!(text.contains("● jump"), "{text}");
        assert!(text.contains("(held)"), "{text}");
        assert!(text.contains("key F8 down"), "the log: {text}");
    }

    #[test]
    fn f3_toggles_the_debug_overlay_and_profiler_records() {
        let config = EngineConfig {
            audio_output: ab_audio::AudioOutput::Silent,
            ..Default::default()
        };
        let mut engine = Engine::new(&config);
        let key = |pressed| PlatformEvent::Key {
            code: KeyCode::F3,
            pressed,
            repeat: false,
        };
        engine.update(1.0 / 60.0);
        assert!(engine
            .tree()
            .find_child(engine.tree().root(), "DebugOverlay", false)
            .is_none());

        engine.handle_event(&key(true));
        engine.update(1.0 / 60.0);
        let tree = engine.tree();
        let layer = tree
            .find_child(tree.root(), "DebugOverlay", false)
            .expect("overlay created on first show");
        assert!(tree.is_visible(layer));
        let text = tree
            .get_component::<ab_ui::Label>(layer, "Panel/Text")
            .unwrap()
            .text
            .clone();
        assert!(text.starts_with("FPS"), "{text}");
        assert!(
            text.contains("systems:") && text.contains("nodes"),
            "{text}"
        );

        engine.handle_event(&key(false));
        engine.update(1.0 / 60.0);
        engine.handle_event(&key(true));
        engine.update(1.0 / 60.0);
        assert!(!engine.tree().is_visible(layer), "toggled off");

        let world = engine.tree().world();
        let profiler = world.resource::<Profiler>().unwrap();
        for name in [
            "frame",
            "fixed",
            "update",
            "scripts.process",
            "system.physics",
            "system.ui_input",
        ] {
            assert!(profiler.average_ms(name).is_some(), "{name} recorded");
        }
        assert!(profiler.frame_statistics().fps > 0.0);
    }

    #[test]
    fn close_request_quits() {
        let mut engine = Engine::new(&EngineConfig::default());
        engine.handle_event(&PlatformEvent::CloseRequested);
        assert!(engine.is_quit_requested());
    }
}
