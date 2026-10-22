//! Tweens: animate a node's position, rotation or scale over time with an easing curve
//! (Godot's `create_tween()`), in sequence or together, once, looping or back and forth.
//!
//! ```
//! use ab_engine::prelude::*;
//! use ab_engine::tween::{Repeat, TweenNodes};
//!
//! let mut tree = SceneTree::new();
//! let coin = tree.create_node3d("Coin");
//! tree.add_child(tree.root(), coin).unwrap();
//! tree.tween(coin)
//!     .easing(Easing::SineInOut)
//!     .repeat(Repeat::PingPong)
//!     .move_by(Vec3::Y * 0.3, 0.8); // bob up and down forever
//! tree.tween(coin)
//!     .easing(Easing::BackOut)
//!     .scale_to(Vec3::splat(1.5), 0.2)
//!     .then()
//!     .scale_to(Vec3::ONE, 0.3); // pop, then settle
//! assert!(tree.is_tweening(coin));
//! ```
//!
//! Tweens run in `Update` with the game's (scaled) time, set the node's local transform, and
//! emit [`TWEEN_FINISHED`] on the node when one that does not repeat ends. They stop when the
//! node is freed, or with [`TweenNodes::kill_tweens`].

use ab_core::{Quat, Time, Vec3};
use ab_ecs::World;
use ab_scene::{NodeHandle, NodeInfo, SceneTree, Signal, SignalQueue, Transform};
use ab_ui::Easing;

/// Signal emitted on the node when a non-repeating tween ends.
pub const TWEEN_FINISHED: &str = "tween_finished";

/// What happens at the end of a tween.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Once,
    /// Starts over from the beginning.
    Loop,
    /// Plays backwards, then forwards again.
    PingPong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Position(Vec3),
    PositionBy(Vec3),
    Rotation(Quat),
    Scale(Vec3),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Start {
    Position(Vec3),
    Rotation(Quat),
    Scale(Vec3),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Tween {
    node: NodeHandle,
    target: Target,
    /// Captured when the tween starts (after its delay).
    start: Option<Start>,
    delay: f32,
    duration: f32,
    elapsed: f32,
    easing: Easing,
    repeat: Repeat,
    forward: bool,
}

/// ECS resource: the running tweens.
#[derive(Debug, Clone, Default)]
pub struct Tweens {
    tweens: Vec<Tween>,
}

impl Tweens {
    pub fn len(&self) -> usize {
        self.tweens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tweens.is_empty()
    }
}

/// Builds tweens on one node. Steps added one after another play together; [`then`]
/// makes the next ones wait for everything added so far.
///
/// [`then`]: TweenBuilder::then
pub struct TweenBuilder<'a> {
    tree: &'a mut SceneTree,
    node: NodeHandle,
    easing: Easing,
    repeat: Repeat,
    /// Start time of the next step.
    at: f32,
    /// End of the longest step so far.
    end: f32,
}

impl std::fmt::Debug for TweenBuilder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TweenBuilder")
            .field("node", &self.node)
            .field("at", &self.at)
            .finish()
    }
}

impl TweenBuilder<'_> {
    /// Easing of the following steps (default: ease out).
    pub fn easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    /// Repeat mode of the following steps.
    pub fn repeat(mut self, repeat: Repeat) -> Self {
        self.repeat = repeat;
        self
    }

    /// Waits `seconds` before the next steps.
    pub fn delay(mut self, seconds: f32) -> Self {
        self.at += seconds.max(0.0);
        self.end = self.end.max(self.at);
        self
    }

    /// The next steps start when everything before has finished.
    pub fn then(mut self) -> Self {
        self.at = self.end;
        self
    }

    /// Moves to a local position.
    pub fn move_to(self, position: Vec3, seconds: f32) -> Self {
        self.add(Target::Position(position), seconds)
    }

    /// Moves by an offset from wherever the node is when the step starts.
    pub fn move_by(self, offset: Vec3, seconds: f32) -> Self {
        self.add(Target::PositionBy(offset), seconds)
    }

    pub fn rotate_to(self, rotation: Quat, seconds: f32) -> Self {
        self.add(Target::Rotation(rotation), seconds)
    }

    pub fn scale_to(self, scale: Vec3, seconds: f32) -> Self {
        self.add(Target::Scale(scale), seconds)
    }

    fn add(mut self, target: Target, seconds: f32) -> Self {
        let duration = seconds.max(1e-4);
        let tween = Tween {
            node: self.node,
            target,
            start: None,
            delay: self.at,
            duration,
            elapsed: 0.0,
            easing: self.easing,
            repeat: self.repeat,
            forward: true,
        };
        self.end = self.end.max(self.at + duration);
        self.tree
            .world_mut()
            .resource_or_insert_with(Tweens::default)
            .tweens
            .push(tween);
        self
    }
}

/// Tween helpers on [`SceneTree`] (`use ab_engine::tween::TweenNodes;`).
pub trait TweenNodes {
    /// Starts building tweens for a 3D node.
    fn tween(&mut self, node: NodeHandle) -> TweenBuilder<'_>;
    /// Stops the node's tweens where they are. Returns how many were stopped.
    fn kill_tweens(&mut self, node: NodeHandle) -> usize;
    fn is_tweening(&self, node: NodeHandle) -> bool;
}

impl TweenNodes for SceneTree {
    fn tween(&mut self, node: NodeHandle) -> TweenBuilder<'_> {
        TweenBuilder {
            tree: self,
            node,
            easing: Easing::EaseOut,
            repeat: Repeat::Once,
            at: 0.0,
            end: 0.0,
        }
    }

    fn kill_tweens(&mut self, node: NodeHandle) -> usize {
        let Some(mut tweens) = self.world().resource_mut::<Tweens>() else {
            return 0;
        };
        let before = tweens.tweens.len();
        tweens.tweens.retain(|t| t.node != node);
        before - tweens.tweens.len()
    }

    fn is_tweening(&self, node: NodeHandle) -> bool {
        self.world()
            .resource::<Tweens>()
            .is_some_and(|t| t.tweens.iter().any(|t| t.node == node))
    }
}

fn capture(transform: &Transform, target: Target) -> Start {
    match target {
        Target::Position(_) | Target::PositionBy(_) => Start::Position(transform.local.translation),
        Target::Rotation(_) => Start::Rotation(transform.local.rotation),
        Target::Scale(_) => Start::Scale(transform.local.scale),
    }
}

fn apply(transform: &mut Transform, start: Start, target: Target, t: f32) {
    match (start, target) {
        (Start::Position(from), Target::Position(to)) => {
            transform.local.translation = from.lerp(to, t);
        }
        (Start::Position(from), Target::PositionBy(offset)) => {
            transform.local.translation = from + offset * t;
        }
        (Start::Rotation(from), Target::Rotation(to)) => {
            transform.local.rotation = from.slerp(to, t);
        }
        (Start::Scale(from), Target::Scale(to)) => {
            transform.local.scale = from.lerp(to, t);
        }
        _ => {}
    }
    transform.dirty = true;
}

/// Advances every tween by the frame's delta. The engine runs it in `Update`.
pub fn tween_system(world: &mut World) {
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    let Some(mut tweens) = world.resource_mut::<Tweens>() else {
        return;
    };
    if tweens.tweens.is_empty() {
        return;
    }
    let mut finished = Vec::new();
    tweens.tweens.retain_mut(|tween| {
        let alive = world
            .get::<NodeInfo>(tween.node.entity())
            .is_some_and(|i| !i.queued_for_deletion);
        let Some(mut transform) = world
            .get_mut::<Transform>(tween.node.entity())
            .filter(|_| alive)
        else {
            return false;
        };
        let mut step = delta;
        if tween.delay > 0.0 {
            let waited = step.min(tween.delay);
            tween.delay -= waited;
            step -= waited;
            if tween.delay > 0.0 {
                return true;
            }
        }
        let start = *tween
            .start
            .get_or_insert_with(|| capture(&transform, tween.target));
        tween.elapsed += step;
        let mut keep = true;
        if tween.elapsed >= tween.duration {
            match tween.repeat {
                Repeat::Once => {
                    tween.elapsed = tween.duration;
                    keep = false;
                    finished.push(tween.node);
                }
                Repeat::Loop => tween.elapsed %= tween.duration,
                Repeat::PingPong => {
                    tween.elapsed %= tween.duration;
                    tween.forward = !tween.forward;
                }
            }
        }
        let progress = tween.elapsed / tween.duration;
        let progress = if tween.forward {
            progress
        } else {
            1.0 - progress
        };
        apply(
            &mut transform,
            start,
            tween.target,
            tween.easing.apply(progress),
        );
        keep
    });
    drop(tweens);
    if !finished.is_empty() {
        if let Some(mut queue) = world.resource_mut::<SignalQueue>() {
            for node in finished {
                queue.push(Signal::new(node, TWEEN_FINISHED));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> SceneTree {
        let mut tree = SceneTree::new();
        tree.world_mut().insert_resource(Time::new(1.0 / 60.0));
        tree
    }

    /// Advances `seconds` in 0.1 s frames (frames are clamped to `Time::MAX_DELTA`).
    fn step(tree: &mut SceneTree, seconds: f32) {
        let frames = (seconds / 0.1).round() as usize;
        for _ in 0..frames {
            tree.world()
                .resource_mut::<Time>()
                .unwrap()
                .begin_frame(0.1);
            tween_system(tree.world_mut());
            tree.process(0.1);
        }
    }

    #[test]
    fn sequences_delays_and_signals() {
        let mut tree = tree();
        let node = tree.create_node3d("N");
        tree.add_child(tree.root(), node).unwrap();
        tree.tween(node)
            .easing(Easing::Linear)
            .move_to(Vec3::new(2.0, 0.0, 0.0), 1.0)
            .then()
            .delay(0.5)
            .scale_to(Vec3::splat(3.0), 1.0);
        step(&mut tree, 0.5);
        assert!(
            (tree.position(node).unwrap().x - 1.0).abs() < 1e-4,
            "halfway"
        );
        assert_eq!(tree.scale(node).unwrap(), Vec3::ONE, "scale waits");
        step(&mut tree, 0.5);
        assert_eq!(tree.position(node).unwrap().x, 2.0);
        step(&mut tree, 1.0); // 0.5 s of delay, then half the scale
        assert!((tree.scale(node).unwrap().x - 2.0).abs() < 1e-4);
        step(&mut tree, 0.6);
        assert_eq!(tree.scale(node).unwrap(), Vec3::splat(3.0));
        assert!(!tree.is_tweening(node));
    }

    #[test]
    fn ping_pong_relative_moves_and_kill() {
        let mut tree = tree();
        let node = tree.create_node3d("N");
        tree.add_child(tree.root(), node).unwrap();
        tree.set_position(node, Vec3::new(0.0, 1.0, 0.0)).unwrap();
        tree.tween(node)
            .easing(Easing::Linear)
            .repeat(Repeat::PingPong)
            .move_by(Vec3::Y, 1.0);
        step(&mut tree, 1.0);
        assert!((tree.position(node).unwrap().y - 2.0).abs() < 1e-4);
        step(&mut tree, 0.5);
        assert!(
            (tree.position(node).unwrap().y - 1.5).abs() < 1e-4,
            "coming back"
        );
        assert_eq!(tree.kill_tweens(node), 1);
        step(&mut tree, 0.5);
        assert!(
            (tree.position(node).unwrap().y - 1.5).abs() < 1e-4,
            "stopped"
        );

        // Freed nodes drop their tweens.
        tree.tween(node).rotate_to(Quat::from_rotation_y(1.0), 1.0);
        tree.queue_free(node).unwrap();
        step(&mut tree, 0.1);
        assert!(tree.world().resource::<Tweens>().unwrap().is_empty());
    }

    #[test]
    fn easing_curves_hit_their_ends() {
        for easing in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::BackOut,
            Easing::ElasticOut,
            Easing::BounceOut,
            Easing::SineInOut,
        ] {
            assert!(easing.apply(0.0).abs() < 1e-4, "{easing:?} starts at 0");
            assert!(
                (easing.apply(1.0) - 1.0).abs() < 1e-4,
                "{easing:?} ends at 1"
            );
        }
        assert!(Easing::BackOut.apply(0.7) > 1.0, "overshoots");
    }
}
