//! Engine facade. Owns the [`SceneTree`] (and through it the ECS world), the system
//! [`Schedule`] and frame timing, and runs the game loop.
//!
//! Frame order:
//!
//! 1. `PreUpdate` systems
//! 2. 0..n times at the fixed rate: `FixedUpdate` systems, `physics_process`, then
//!    `PostFixedUpdate` systems (the physics step: Rapier ↔ node sync)
//! 3. `Update` systems + `process`, transform propagation, deferred frees
//! 4. `PostUpdate` (audio: listener, players, 3D positions), `Render`, `Cleanup` systems
//! 5. per-frame input state is cleared
//! 6. the UI is laid out for the window size, then the window renderer draws the scene
//!    (extraction → shadows → forward → post) and the UI on top
//!
//! ```no_run
//! use ab_engine::{Engine, EngineConfig};
//!
//! fn main() -> anyhow::Result<()> {
//!     ab_engine::run(EngineConfig::default(), |engine: &mut Engine| {
//!         let tree = engine.tree_mut();
//!         let player = tree.create_node3d("Player");
//!         tree.add_child(tree.root(), player)?;
//!         Ok(())
//!     })
//! }
//! ```

pub mod animation_file;
mod app;
pub mod assets;
pub mod blood;
pub mod climb;
mod config;
pub mod crash;
mod debug;
mod debug_overlay;
mod engine;
pub mod game;
pub mod game_config;
pub mod head_bob;
mod hot_reload;
pub mod input_overlay;
pub mod localization;
pub mod procgen;
mod profiler;
pub mod ragdoll;
pub mod save;
pub mod scene_file;
pub mod spline;
pub mod script_host;
pub mod trace;
pub mod tween;

pub use app::{render_offscreen, run, run_headless};

/// True in builds for players (the `shipping` feature, set by `cargo xtask export`): debug
/// keys and developer options are off.
pub const SHIPPING: bool = cfg!(feature = "shipping");
pub use config::EngineConfig;
pub use debug::{debug_shapes_system, profile, DebugSettings, BOUNDS_COLOR, SKINNED_BOUNDS_COLOR};
pub use debug_overlay::DebugOverlay;
pub use engine::{Engine, EngineControl, RenderStats};
pub use game::{change_scene, reload_scene, run_game, CurrentScene, Game, GameSettings};
pub use input_overlay::InputOverlay;
pub use profiler::{
    FrameSample, FrameStatistics, ProfileEntry, Profiler, HISTORY as PROFILE_HISTORY,
};
pub use trace::{TraceOverlay, Tracer};

/// Commonly used types, for `use ab_engine::prelude::*`.
pub mod prelude {
    pub use crate::head_bob::HeadBob;
    pub use crate::save::SaveStore;
    pub use crate::scene_file::{load_scene, save_scene, SceneFile};
    pub use crate::trace::{Tracer, Tracing};
    pub use crate::tween::{Repeat, TweenNodes};
    pub use crate::{
        change_scene, profile, reload_scene, run_game, DebugOverlay, DebugSettings, Engine,
        EngineConfig, EngineControl, Game, InputOverlay, Profiler, RenderStats,
    };
    pub use ab_animation::{AnimationLibrary, AnimationPlayer, Locomotion, LocomotionState};
    pub use ab_asset::{spawn_gltf, AssetServer};
    pub use ab_audio::{
        Attenuation, Audio, AudioListener3D, AudioNodes, AudioPlayer, AudioStream, SoundOptions,
    };
    pub use ab_core::{Color, Quat, Time, Transform3D, Vec2, Vec3};
    pub use ab_ecs::{Entity, ScheduleStage, World};
    pub use ab_effects::{
        flash_screen, hit_stop, CameraShake, ColorRamp, EffectNodes, EmissionShape,
        ExplosionOptions, LightFlash, ParticleEmitter, ParticleSettings, ScreenEffects,
    };
    pub use ab_input::{
        Actions, Binding, GamepadButton, InputMap, InputState, KeyCode, MouseButton,
    };
    pub use ab_navigation::{NavBakeSettings, NavGrid, PathFollower};
    pub use ab_physics::{
        BodyKind, CharacterBody3D, CollisionShape3D, Joint3D, JointKind, PhysicsBody3D,
        PhysicsEvent, PhysicsNodes, PhysicsWorld, QueryOptions, Ragdoll, RayHit, Shape,
        SkeletonRagdoll,
    };
    pub use ab_rendering::{
        Aabb, Bloom, Bounds, Camera3D, DebugDraw, DirectionalLight3D, Environment, Material, Mesh,
        MeshInstance3D, OmniLight3D, RenderAssets, RenderNodes, SpotLight3D,
    };
    pub use ab_scene::{
        serde_script_state, NodeHandle, NodeScript, SceneTree, ScriptRegistry, Signal, Timer,
    };
    pub use ab_ui::{
        BoxContainer, Button, CanvasLayer, Control, Easing, FontId, Fonts, HAlign, Label,
        LayoutPreset, MouseFilter, Panel, ProgressBar, StyleBox, TextShadow, TextStyle,
        TextureRect, TweenTarget, UiEvent, UiNodes, UiSettings, UiState, VAlign,
    };
}

pub use ab_animation as animation;
pub use ab_asset as asset;
pub use ab_audio as audio;
pub use ab_core as core;
pub use ab_ecs as ecs;
pub use ab_effects as effects;
pub use ab_input as input;
pub use ab_navigation as navigation;
pub use ab_physics as physics;
pub use ab_platform as platform;
pub use ab_rendering as rendering;
pub use ab_scene as scene;
pub use ab_ui as ui;
pub use ab_vfs as vfs;
/// Logging for games (`ab_engine::log::info!(...)`); shown in the console and the editor.
pub use tracing as log;
