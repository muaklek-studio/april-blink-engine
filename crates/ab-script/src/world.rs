//! Effects, tweens, audio, navigation, timers, lights and cameras.

use crate::host::method;
use crate::math::{Quat, Vec3};
use crate::physics::property;
use crate::tree::Tree;
use crate::ui::Easing;
use crate::value::{Color, Error, IntoValue, Node};
use ab_script_api as ffi;

// =============================================================================================
// Effects
// =============================================================================================

/// A one-shot particle effect: a preset with optional changes ([`Tree::spawn_effect`]).
///
/// Presets: `smoke`, `fire`, `embers`, `sparks`, `dust`, `magic`, `wisps`, `explosion`,
/// `rain`, `snow`, `heal`, `fireflies`, `splash`, `leaves`, `blood`, `blood_mist`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleEffect {
    preset: String,
    amount: Option<u32>,
    lifetime: Option<f32>,
    size: Option<(f32, f32)>,
    speed: Option<(f32, f32)>,
    colors: Option<(Color, Color)>,
    direction: Option<(Vec3, f32)>,
}

impl ParticleEffect {
    pub fn preset(name: &str) -> Self {
        Self {
            preset: name.to_owned(),
            amount: None,
            lifetime: None,
            size: None,
            speed: None,
            colors: None,
            direction: None,
        }
    }

    /// Particles emitted at once.
    pub fn with_amount(mut self, amount: u32) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Seconds each particle lives.
    pub fn with_lifetime(mut self, seconds: f32) -> Self {
        self.lifetime = Some(seconds);
        self
    }

    /// Size at birth and at death.
    pub fn with_size(mut self, start: f32, end: f32) -> Self {
        self.size = Some((start, end));
        self
    }

    /// Random initial speed range (m/s).
    pub fn with_speed(mut self, min: f32, max: f32) -> Self {
        self.speed = Some((min, max));
        self
    }

    /// Color at birth and at death.
    pub fn with_colors(mut self, start: Color, end: Color) -> Self {
        self.colors = Some((start, end));
        self
    }

    /// Emits around `direction`, within `spread` degrees.
    pub fn with_direction(mut self, direction: Vec3, spread: f32) -> Self {
        self.direction = Some((direction, spread));
        self
    }

    fn args(&self) -> Vec<ffi::Value> {
        let pair = |p: Option<(f32, f32)>| [p.map(|p| p.0).to_value(), p.map(|p| p.1).to_value()];
        let mut args = vec![
            self.preset.to_value(),
            self.amount.to_value(),
            self.lifetime.to_value(),
        ];
        args.extend(pair(self.size));
        args.extend(pair(self.speed));
        args.push(self.colors.map(|c| c.0).to_value());
        args.push(self.colors.map(|c| c.1).to_value());
        args.push(self.direction.map(|d| d.0).to_value());
        args.push(self.direction.map(|d| d.1).to_value());
        args
    }
}

/// An explosion: fireball, smoke, sparks, a flash of light and camera shake.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Explosion {
    /// Overall size multiplier.
    pub scale: f32,
    /// Peak energy of the flash light (0 = none).
    pub flash: f32,
    /// Trauma added to every camera shake.
    pub shake: f32,
    /// Screen distortion kick (0 = none).
    pub distortion: f32,
}

/// Blood from a wound: droplets, a fine mist and splats where the droplets land
/// ([`Tree::spawn_blood`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blood {
    /// Which way the blood leaves the wound: the hit's direction, or the surface normal.
    pub direction: Vec3,
    /// How much: 1 a normal hit, 2-3 a heavy one, 0.5 a graze.
    pub amount: f32,
    /// Splats left on nearby surfaces (0 = none).
    pub splats: u32,
    /// Seconds a splat stays before it dries away (`None`: the engine's 20 s).
    pub splat_lifetime: Option<f32>,
    /// Colour of fresh blood (`None`: dark red).
    pub color: Option<Color>,
}

impl Blood {
    /// Blood thrown along `direction`.
    pub fn along(direction: Vec3) -> Self {
        Self {
            direction,
            ..Self::default()
        }
    }
}

impl Default for Blood {
    fn default() -> Self {
        Self {
            direction: Vec3::Y,
            amount: 1.0,
            splats: 4,
            splat_lifetime: None,
            color: None,
        }
    }
}

impl Default for Explosion {
    fn default() -> Self {
        Self {
            scale: 1.0,
            flash: 40.0,
            shake: 0.6,
            distortion: 1.0,
        }
    }
}

// =============================================================================================
// Tweens
// =============================================================================================

/// What happens at the end of a tween.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Repeat {
    #[default]
    Once,
    /// Starts over from the beginning.
    Loop,
    /// Plays backwards, then forwards again.
    PingPong,
}

/// Builds tweens of a 3D node's transform ([`Tree::tween`]); they start when the builder is
/// dropped (the end of the statement).
///
/// ```ignore
/// tree.tween(coin)
///     .easing(Easing::BackOut)
///     .scale_to(Vec3::splat(1.5), 0.2)
///     .then()
///     .scale_to(Vec3::ONE, 0.3); // pop, then settle
/// ```
#[derive(Debug)]
#[must_use = "a tween starts when the builder is dropped; chain steps onto it"]
pub struct TweenBuilder<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Easing(Easing),
    Repeat(Repeat),
    Delay(f32),
    Then,
    MoveTo(Vec3, f32),
    MoveBy(Vec3, f32),
    RotateTo(Quat, f32),
    ScaleTo(Vec3, f32),
}

impl TweenBuilder<'_, '_> {
    /// Easing of the following steps.
    pub fn easing(mut self, easing: Easing) -> Self {
        self.steps.push(Step::Easing(easing));
        self
    }

    /// What the tweens do when they end.
    pub fn repeat(mut self, repeat: Repeat) -> Self {
        self.steps.push(Step::Repeat(repeat));
        self
    }

    /// Waits before the following steps.
    pub fn delay(mut self, seconds: f32) -> Self {
        self.steps.push(Step::Delay(seconds));
        self
    }

    /// The following steps start when the previous ones end (otherwise they run together).
    pub fn then(mut self) -> Self {
        self.steps.push(Step::Then);
        self
    }

    pub fn move_to(mut self, position: Vec3, seconds: f32) -> Self {
        self.steps.push(Step::MoveTo(position, seconds));
        self
    }

    pub fn move_by(mut self, offset: Vec3, seconds: f32) -> Self {
        self.steps.push(Step::MoveBy(offset, seconds));
        self
    }

    pub fn rotate_to(mut self, rotation: Quat, seconds: f32) -> Self {
        self.steps.push(Step::RotateTo(rotation, seconds));
        self
    }

    pub fn scale_to(mut self, scale: Vec3, seconds: f32) -> Self {
        self.steps.push(Step::ScaleTo(scale, seconds));
        self
    }
}

impl Drop for TweenBuilder<'_, '_> {
    fn drop(&mut self) {
        let mut args = vec![self.node.to_value()];
        for step in &self.steps {
            match *step {
                Step::Easing(easing) => {
                    args.extend(["easing".to_value(), easing.name().to_value()])
                }
                Step::Repeat(repeat) => {
                    let name = match repeat {
                        Repeat::Once => "once",
                        Repeat::Loop => "loop",
                        Repeat::PingPong => "ping_pong",
                    };
                    args.extend(["repeat".to_value(), name.to_value()]);
                }
                Step::Delay(seconds) => args.extend(["delay".to_value(), seconds.to_value()]),
                Step::Then => args.extend(["then".to_value(), ffi::Value::NIL]),
                Step::MoveTo(v, s) => {
                    args.extend(["move_to".to_value(), v.to_value(), s.to_value()]);
                }
                Step::MoveBy(v, s) => {
                    args.extend(["move_by".to_value(), v.to_value(), s.to_value()]);
                }
                Step::RotateTo(q, s) => {
                    args.extend(["rotate_to".to_value(), q.to_value(), s.to_value()]);
                }
                Step::ScaleTo(v, s) => {
                    args.extend(["scale_to".to_value(), v.to_value(), s.to_value()]);
                }
            }
        }
        if let Err(err) = self.tree.run(method!("tween.start"), &args) {
            crate::host::log(
                ffi::LogLevel::WARN,
                "ab_script",
                format_args!("tween not started: {err}"),
            );
        }
    }
}

/// Signal a node emits when a tween that does not repeat ends.
pub const TWEEN_FINISHED: &str = "tween_finished";

// =============================================================================================
// Audio, navigation, timers
// =============================================================================================

/// How a one-shot sound plays ([`Tree::play_sound`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundOptions {
    /// Loudness in decibels (0 = as recorded, -6 = about half).
    pub volume_db: f32,
    /// Playback speed (also changes the pitch).
    pub pitch_scale: f32,
}

impl Default for SoundOptions {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pitch_scale: 1.0,
        }
    }
}

/// Settings for [`Tree::bake_navigation`].
#[derive(Debug, Clone, PartialEq)]
pub struct NavBakeSettings {
    /// The box to bake (world space).
    pub min: Vec3,
    pub max: Vec3,
    /// Grid resolution (m).
    pub cell_size: f32,
    /// Agents keep this far from walls (m).
    pub agent_radius: f32,
    pub agent_height: f32,
    /// Highest step an agent climbs (m).
    pub max_step: f32,
    /// Layers that are walkable floors and obstacles.
    pub collision_mask: u32,
}

impl Default for NavBakeSettings {
    fn default() -> Self {
        Self {
            min: Vec3::new(-20.0, -1.0, -20.0),
            max: Vec3::new(20.0, 10.0, 20.0),
            cell_size: 0.25,
            agent_radius: 0.35,
            agent_height: 1.8,
            max_step: 0.3,
            collision_mask: u32::MAX,
        }
    }
}

/// Signal a timer emits when it runs out.
pub const TIMEOUT: &str = "timeout";

impl<'a> Tree<'a> {
    /// Plays a one-shot particle effect at `position` under `parent`; freed when done.
    pub fn spawn_effect(
        &mut self,
        parent: Node,
        position: Vec3,
        effect: &ParticleEffect,
    ) -> Result<Node, Error> {
        let mut args = vec![parent.to_value(), position.to_value()];
        args.extend(effect.args());
        self.created(method!("effects.spawn"), &args)
    }

    /// Plays an effect scene made in the editor's Effect editor (`effects/blast.toml`) at
    /// `position` under `parent`; freed once its one-shot layers and flashes are done.
    pub fn spawn_effect_scene(
        &mut self,
        parent: Node,
        position: Vec3,
        path: &str,
    ) -> Result<Node, Error> {
        self.created(
            method!("effects.spawn_scene"),
            &[parent.to_value(), position.to_value(), path.to_value()],
        )
    }

    /// An explosion at `position` under `parent`; freed when done.
    pub fn spawn_explosion(
        &mut self,
        parent: Node,
        position: Vec3,
        explosion: Explosion,
    ) -> Result<Node, Error> {
        self.created(
            method!("effects.explosion"),
            &[
                parent.to_value(),
                position.to_value(),
                explosion.scale.to_value(),
                explosion.flash.to_value(),
                explosion.shake.to_value(),
                explosion.distortion.to_value(),
            ],
        )
    }

    /// Blood at `position` under `parent` (particles freed when done; splats on the
    /// surfaces around fade on their own).
    pub fn spawn_blood(
        &mut self,
        parent: Node,
        position: Vec3,
        blood: Blood,
    ) -> Result<Node, Error> {
        self.created(
            method!("effects.blood"),
            &[
                parent.to_value(),
                position.to_value(),
                blood.direction.to_value(),
                blood.amount.to_value(),
                blood.splats.to_value(),
                blood.splat_lifetime.to_value(),
                blood.color.to_value(),
            ],
        )
    }

    /// Shakes every camera under a `CameraShake` node (0..1 trauma; more for big hits).
    pub fn shake_cameras(&mut self, trauma: f32) {
        let _ = self.run(method!("effects.shake_cameras"), &[trauma.to_value()]);
    }

    /// Tweens the node's transform; see [`TweenBuilder`].
    pub fn tween(&mut self, node: Node) -> TweenBuilder<'_, 'a> {
        TweenBuilder {
            tree: self,
            node,
            steps: Vec::new(),
        }
    }

    /// Stops the node's tweens where they are.
    pub fn kill_tweens(&mut self, node: Node) {
        let _ = self.run(method!("tween.kill"), &[node.to_value()]);
    }

    pub fn is_tweening(&self, node: Node) -> bool {
        self.query(method!("tween.is_tweening"), &[node.to_value()])
            .unwrap_or(false)
    }

    /// Plays a sound file (`"audio/click.wav"`) once, not positioned.
    pub fn play_sound(&mut self, path: &str, options: SoundOptions) -> Result<(), Error> {
        self.run(
            method!("audio.play"),
            &[
                path.to_value(),
                options.volume_db.to_value(),
                options.pitch_scale.to_value(),
            ],
        )
    }

    /// Plays a sound file once at a point in the world (louder near the listener).
    pub fn play_sound_at(
        &mut self,
        path: &str,
        position: Vec3,
        options: SoundOptions,
    ) -> Result<(), Error> {
        self.run(
            method!("audio.play_at"),
            &[
                path.to_value(),
                position.to_value(),
                options.volume_db.to_value(),
                options.pitch_scale.to_value(),
            ],
        )
    }

    /// Loudness of everything, in decibels.
    pub fn master_volume_db(&self) -> f32 {
        self.query(method!("audio.master_volume_db"), &[])
            .unwrap_or(0.0)
    }

    pub fn set_master_volume_db(&mut self, volume_db: f32) {
        let _ = self.run(
            method!("audio.set_master_volume_db"),
            &[volume_db.to_value()],
        );
    }

    /// Bakes the walkable areas from the physics world (floors and obstacles in
    /// `settings.collision_mask`); replaces the previous navigation. Returns the number of
    /// walkable cells. The level must have been stepped by physics once.
    pub fn bake_navigation(&mut self, settings: &NavBakeSettings) -> Result<usize, Error> {
        Ok(self
            .fetch(
                method!("nav.bake"),
                &[
                    settings.min.to_value(),
                    settings.max.to_value(),
                    settings.cell_size.to_value(),
                    settings.agent_radius.to_value(),
                    settings.agent_height.to_value(),
                    settings.max_step.to_value(),
                    settings.collision_mask.to_value(),
                ],
            )?
            .unwrap_or(0))
    }

    /// Removes the baked navigation.
    pub fn clear_navigation(&mut self) {
        let _ = self.run(method!("nav.clear"), &[]);
    }

    pub fn has_navigation(&self) -> bool {
        self.query(method!("nav.has_grid"), &[]).unwrap_or(false)
    }

    /// Waypoints from `from` to `to` over the baked navigation, or `None` without a path.
    pub fn find_path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let mut found = false;
        let mut path = Vec::new();
        self.call(
            method!("nav.find_path"),
            &[from.to_value(), to.to_value()],
            |v| {
                found = true;
                path.extend(<Vec3 as crate::value::FromValue>::from_value(v));
            },
        )
        .ok()?;
        found.then_some(path)
    }

    /// A timer node (detached; add it to the tree). It emits [`TIMEOUT`] when it runs out.
    pub fn create_timer(
        &mut self,
        name: &str,
        wait_time: f32,
        one_shot: bool,
        autostart: bool,
    ) -> Result<Node, Error> {
        self.created(
            method!("timer.create"),
            &[
                name.to_value(),
                wait_time.to_value(),
                one_shot.to_value(),
                autostart.to_value(),
            ],
        )
    }

    /// The node's `Timer`.
    pub fn timer(&mut self, node: Node) -> TimerRef<'_, 'a> {
        TimerRef { tree: self, node }
    }

    /// The node's light (omni, spot or directional).
    pub fn light(&mut self, node: Node) -> LightRef<'_, 'a> {
        LightRef { tree: self, node }
    }

    /// Sends `damage` (`amount`) to `target` like `emit_with(target, "damage", amount)`, and
    /// records `source` as the attacker in the engine's damage trace (F9, the editor's Debug
    /// panel).
    pub fn deal_damage(
        &mut self,
        source: Option<Node>,
        target: Node,
        amount: f32,
    ) -> Result<(), Error> {
        self.run(
            method!("trace.damage"),
            &[source.to_value(), target.to_value(), amount.to_value()],
        )
    }

    /// Sends `heal` (`amount`) to `target`, recording `source` as the healer.
    pub fn heal(&mut self, source: Option<Node>, target: Node, amount: f32) -> Result<(), Error> {
        self.run(
            method!("trace.heal"),
            &[source.to_value(), target.to_value(), amount.to_value()],
        )
    }

    /// Adds a line to `node`'s history in the trace ("state: chasing", "reloading").
    pub fn trace_note(&mut self, node: Node, text: &str) -> Result<(), Error> {
        self.run(method!("trace.note"), &[node.to_value(), text.to_value()])
    }

    /// Follows `node` in the trace: its trail, speed and every signal it emits.
    pub fn trace_watch(&mut self, node: Node, watch: bool) -> Result<(), Error> {
        self.run(method!("trace.watch"), &[node.to_value(), watch.to_value()])
    }

    /// Reports `node`'s health to the trace (health scripts call it after every change).
    pub fn report_health(&mut self, node: Node, current: f32, max: f32) -> Result<(), Error> {
        self.run(
            method!("trace.health"),
            &[node.to_value(), current.to_value(), max.to_value()],
        )
    }

    /// Makes the 3D node `node` (a pivot between the character and its camera) move like a
    /// head: bobbing with the steps, swaying, tilting into strafes, dipping on landings,
    /// from the character's real movement. `strength`: 1 natural, 0.5 subtle, 0 off.
    /// `follow`: a head bone to ride on (full-body first person).
    pub fn set_head_bob(
        &mut self,
        node: Node,
        strength: f32,
        follow: Option<Node>,
    ) -> Result<(), Error> {
        self.run(
            method!("head_bob.set"),
            &[node.to_value(), strength.to_value(), follow.to_value()],
        )
    }

    /// The animations of the model at `node`: play, crossfade, stop.
    pub fn animation(&mut self, node: Node) -> AnimationRef<'_, 'a> {
        AnimationRef { tree: self, node }
    }

    /// The locomotion of the model at `node`: clips picked and blended from movement.
    pub fn locomotion(&mut self, node: Node) -> LocomotionRef<'_, 'a> {
        LocomotionRef { tree: self, node }
    }

    /// The node's `Camera3D`.
    pub fn camera(&mut self, node: Node) -> CameraRef<'_, 'a> {
        CameraRef { tree: self, node }
    }

    /// The node's audio player.
    pub fn audio_player(&mut self, node: Node) -> AudioPlayerRef<'_, 'a> {
        AudioPlayerRef { tree: self, node }
    }

    /// The node's `MeshInstance3D`.
    pub fn mesh(&mut self, node: Node) -> MeshRef<'_, 'a> {
        MeshRef { tree: self, node }
    }

    /// The node's `Particles3D` emitter.
    pub fn particles(&mut self, node: Node) -> ParticlesRef<'_, 'a> {
        ParticlesRef { tree: self, node }
    }
}

/// A node's `Timer` ([`Tree::timer`]).
#[derive(Debug)]
pub struct TimerRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl TimerRef<'_, '_> {
    property!(
        wait_time,
        set_wait_time,
        f32,
        "timer.wait_time",
        "Seconds per run."
    );
    property!(
        one_shot,
        set_one_shot,
        bool,
        "timer.one_shot",
        "Stops after one run (otherwise restarts)."
    );

    /// Starts (or restarts) the countdown.
    pub fn start(&mut self) -> Result<(), Error> {
        self.tree
            .run(method!("timer.start"), &[self.node.to_value()])
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        self.tree
            .run(method!("timer.stop"), &[self.node.to_value()])
    }

    pub fn time_left(&self) -> f32 {
        self.tree.get(self.node, "timer.time_left").unwrap_or(0.0)
    }

    pub fn is_stopped(&self) -> bool {
        self.tree.get(self.node, "timer.stopped").unwrap_or(true)
    }
}

/// A model's animations ([`Tree::animation`]): `node` is the model, or any node above its
/// animation player.
#[derive(Debug)]
pub struct AnimationRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl AnimationRef<'_, '_> {
    /// Plays the clip `name` from its start, looping.
    pub fn play(&mut self, name: &str) -> Result<(), Error> {
        self.crossfade(name, 0.0)
    }

    /// Plays the clip `name` once (it holds its last pose).
    pub fn play_once(&mut self, name: &str) -> Result<(), Error> {
        self.tree.run(
            method!("animation.play"),
            &[
                self.node.to_value(),
                name.to_value(),
                0.0f32.to_value(),
                false.to_value(),
            ],
        )
    }

    /// Plays the clip `name`, blending from the current pose over `seconds`.
    pub fn crossfade(&mut self, name: &str, seconds: f32) -> Result<(), Error> {
        self.tree.run(
            method!("animation.play"),
            &[
                self.node.to_value(),
                name.to_value(),
                seconds.to_value(),
                true.to_value(),
            ],
        )
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        self.tree
            .run(method!("animation.stop"), &[self.node.to_value()])
    }

    /// Playback rate: 1 normal, 2 twice as fast, negative backwards.
    pub fn set_speed(&mut self, speed: f32) -> Result<(), Error> {
        self.tree.run(
            method!("animation.set_speed"),
            &[self.node.to_value(), speed.to_value()],
        )
    }

    /// The clip playing.
    pub fn current(&self) -> Option<String> {
        self.tree
            .query(method!("animation.current"), &[self.node.to_value()])
    }

    /// Whether a clip is playing (a clip played once has stopped at its end).
    pub fn is_playing(&self) -> bool {
        self.tree
            .query(method!("animation.is_playing"), &[self.node.to_value()])
            .unwrap_or(false)
    }

    /// The names of the model's clips.
    pub fn clips(&self) -> Vec<String> {
        self.tree
            .query::<String>(method!("animation.clips"), &[self.node.to_value()])
            .map(|list| {
                list.split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// What a [`LocomotionRef`] is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocomotionState {
    Ground,
    Jump,
    Fall,
    Climb,
    Action,
}

/// A model's locomotion ([`Tree::locomotion`]): it picks and blends the model's clips from
/// how the character moves. Call [`LocomotionRef::update`] every frame; the clips are the
/// usual `Idle`, `Walk`, `Run`, `Jump`, `Fall`, `Climb` unless [`LocomotionRef::set_clips`]
/// says otherwise.
#[derive(Debug)]
pub struct LocomotionRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl LocomotionRef<'_, '_> {
    /// How the character moves this frame: horizontal speed (m/s), on the ground, vertical
    /// speed (m/s; along the wall when climbing), climbing.
    pub fn update(
        &mut self,
        speed: f32,
        grounded: bool,
        vertical_speed: f32,
        climbing: bool,
    ) -> Result<(), Error> {
        self.tree.run(
            method!("locomotion.update"),
            &[
                self.node.to_value(),
                speed.to_value(),
                grounded.to_value(),
                vertical_speed.to_value(),
                climbing.to_value(),
            ],
        )
    }

    /// Plays `clip` once over the movement (attack, reload, dodge), then goes back to it.
    pub fn play_action(&mut self, clip: &str) -> Result<(), Error> {
        self.tree.run(
            method!("locomotion.action"),
            &[self.node.to_value(), clip.to_value()],
        )
    }

    pub fn is_action_playing(&self) -> bool {
        self.tree
            .query(
                method!("locomotion.action_playing"),
                &[self.node.to_value()],
            )
            .unwrap_or(false)
    }

    pub fn state(&self) -> LocomotionState {
        let state: Option<String> = self
            .tree
            .query(method!("locomotion.state"), &[self.node.to_value()]);
        match state.as_deref() {
            Some("jump") => LocomotionState::Jump,
            Some("fall") => LocomotionState::Fall,
            Some("climb") => LocomotionState::Climb,
            Some("action") => LocomotionState::Action,
            _ => LocomotionState::Ground,
        }
    }

    /// The clips to use: ground clips with the speed (m/s) each was made for
    /// (`&[("Idle", 0.0), ("Walk", 1.5), ("Run", 4.2)]`), and the jump, fall and climb clips
    /// (`None`: not used).
    pub fn set_clips(
        &mut self,
        ground: &[(&str, f32)],
        jump: Option<&str>,
        fall: Option<&str>,
        climb: Option<&str>,
    ) -> Result<(), Error> {
        let ground: Vec<String> = ground
            .iter()
            .map(|(name, speed)| format!("{name}:{speed}"))
            .collect();
        let or_empty = |s: Option<&str>| s.unwrap_or("").to_owned();
        self.tree.run(
            method!("locomotion.set_clips"),
            &[
                self.node.to_value(),
                ground.join(",").as_str().to_value(),
                or_empty(jump).as_str().to_value(),
                or_empty(fall).as_str().to_value(),
                or_empty(climb).as_str().to_value(),
            ],
        )
    }
}

/// A node's light ([`Tree::light`]).
#[derive(Debug)]
pub struct LightRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl LightRef<'_, '_> {
    property!(energy, set_energy, f32, "light.energy", "Brightness.");
    property!(color, set_color, Color, "light.color", "Light color.");
    property!(
        range,
        set_range,
        f32,
        "light.range",
        "Reach in metres (omni and spot lights)."
    );
    property!(
        shadows,
        set_shadows,
        bool,
        "light.shadows",
        "Casts shadows."
    );
}

/// A node's `Camera3D` ([`Tree::camera`]).
#[derive(Debug)]
pub struct CameraRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl CameraRef<'_, '_> {
    property!(
        fov,
        set_fov,
        f32,
        "camera.fov",
        "Vertical field of view, degrees."
    );
    property!(
        near,
        set_near,
        f32,
        "camera.near",
        "Nearest visible distance (m)."
    );
    property!(
        far,
        set_far,
        f32,
        "camera.far",
        "Farthest visible distance (m)."
    );

    pub fn is_current(&self) -> bool {
        self.tree.get(self.node, "camera.current").unwrap_or(false)
    }

    /// Renders the view from this camera.
    pub fn make_current(&mut self) -> Result<(), Error> {
        self.tree.set(self.node, "camera.current", true)
    }
}

/// A node's audio player ([`Tree::audio_player`]).
#[derive(Debug)]
pub struct AudioPlayerRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl AudioPlayerRef<'_, '_> {
    property!(
        volume_db,
        set_volume_db,
        f32,
        "audio_player.volume_db",
        "Loudness in decibels."
    );
    property!(
        pitch_scale,
        set_pitch_scale,
        f32,
        "audio_player.pitch_scale",
        "Playback speed."
    );
    property!(
        looping,
        set_looping,
        bool,
        "audio_player.looping",
        "Starts over at the end."
    );

    pub fn play(&mut self) -> Result<(), Error> {
        self.tree.set(self.node, "audio_player.playing", true)
    }

    pub fn stop(&mut self) -> Result<(), Error> {
        self.tree.set(self.node, "audio_player.playing", false)
    }

    pub fn is_playing(&self) -> bool {
        self.tree
            .get(self.node, "audio_player.playing")
            .unwrap_or(false)
    }
}

/// A node's `MeshInstance3D` ([`Tree::mesh`]).
#[derive(Debug)]
pub struct MeshRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl MeshRef<'_, '_> {
    property!(
        color,
        set_color,
        Color,
        "mesh.color",
        "Base color of its material."
    );
    property!(
        emission,
        set_emission,
        Color,
        "mesh.emission",
        "Glow color of its material."
    );
    property!(
        emission_energy,
        set_emission_energy,
        f32,
        "mesh.emission_energy",
        "Glow strength."
    );
    property!(
        cast_shadows,
        set_cast_shadows,
        bool,
        "mesh.cast_shadows",
        "Casts shadows."
    );
}

/// A node's `Particles3D` ([`Tree::particles`]).
#[derive(Debug)]
pub struct ParticlesRef<'t, 'a> {
    tree: &'t mut Tree<'a>,
    node: Node,
}

impl ParticlesRef<'_, '_> {
    property!(
        emitting,
        set_emitting,
        bool,
        "particles.emitting",
        "Emits new particles."
    );
    property!(
        amount,
        set_amount,
        u32,
        "particles.amount",
        "Particles per emission cycle."
    );

    /// Starts emitting again from the beginning (replays one-shot emitters).
    pub fn restart(&mut self) -> Result<(), Error> {
        self.tree
            .run(method!("particles.restart"), &[self.node.to_value()])
    }
}
