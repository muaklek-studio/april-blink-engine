//! Animated character: plays a rigged model's animations from how its body moves (idle,
//! walk, run, jump, fall, climb, blended by speed and crossfaded), and one-shot animations
//! when other scripts signal (attacks, hits), for players and non-player characters alike.
//!
//! Put it on the `CharacterBody3D` (or any moving body) whose child is the model. Clips are
//! found by the usual names (`Idle`, `Walk`, `Run`, `Jump`, `Fall`, `Climb`); set the names
//! and speeds below for other models.
//!
//! Signals it plays a clip for, on the body node: `play_animation` (the clip's name), and
//! `damaged` (plays `hit_clip` when the model has it).

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimatedCharacter {
    /// Ground clips and the speed (m/s) each one was made for.
    pub idle: String,
    pub walk: String,
    pub walk_speed: f32,
    pub run: String,
    pub run_speed: f32,
    pub jump: String,
    pub fall: String,
    pub climb: String,
    /// Played when the body is damaged (if the model has it).
    pub hit_clip: String,
    #[serde(skip)]
    model: Option<Node>,
    #[serde(skip)]
    previous: Option<Vec3>,
}

impl Default for AnimatedCharacter {
    fn default() -> Self {
        Self {
            idle: "Idle".into(),
            walk: "Walk".into(),
            walk_speed: 1.5,
            run: "Run".into(),
            run_speed: 4.2,
            jump: "Jump".into(),
            fall: "Fall".into(),
            climb: "Climb".into(),
            hit_clip: "Hit".into(),
            model: None,
            previous: None,
        }
    }
}

/// Signal another script emits on the body to play a one-shot clip by name.
pub const PLAY_ANIMATION: &str = "play_animation";

impl Script for AnimatedCharacter {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        self.model = tree
            .children(node)
            .into_iter()
            .find(|&c| !tree.animation(c).clips().is_empty());
        let Some(model) = self.model else {
            warn!("AnimatedCharacter: no child of {node} has animations");
            return;
        };
        let clips = tree.animation(model).clips();
        fn has<'n>(clips: &[String], name: &'n str) -> Option<&'n str> {
            clips.iter().any(|c| c == name).then_some(name)
        }
        let has = |name| has(&clips, name);
        let ground: Vec<(&str, f32)> = [
            (self.idle.as_str(), 0.0),
            (self.walk.as_str(), self.walk_speed),
            (self.run.as_str(), self.run_speed),
        ]
        .into_iter()
        .filter(|(name, _)| has(name).is_some())
        .collect();
        let _ = tree.locomotion(model).set_clips(
            &ground,
            has(&self.jump),
            has(&self.fall),
            has(&self.climb),
        );
        let _ = tree.connect(node, PLAY_ANIMATION, node);
        let _ = tree.connect(node, "damaged", node);
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        let Some(model) = self.model else {
            return;
        };
        // Character bodies report their velocity; other bodies by how far they moved.
        let velocity = match tree.character(node).velocity() {
            Some(v) => v,
            None => {
                let at = tree.global_position(node).unwrap_or_default();
                let v = self
                    .previous
                    .map_or(Vec3::ZERO, |p| (at - p) / delta.max(1e-4));
                self.previous = Some(at);
                v
            }
        };
        let climbing = tree.climb(node).is_climbing();
        let grounded = climbing
            || tree.character(node).velocity().is_none()
            || tree.character(node).is_on_floor();
        let _ = tree.locomotion(model).update(
            Vec2::new(velocity.x, velocity.z).length(),
            grounded,
            velocity.y,
            climbing,
        );
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        let Some(model) = self.model else {
            return;
        };
        let clip = if signal.is(PLAY_ANIMATION) {
            signal.arg::<String>()
        } else if signal.is("damaged") {
            Some(self.hit_clip.clone())
        } else {
            None
        };
        if let Some(clip) = clip {
            if tree.animation(model).clips().contains(&clip) {
                let _ = tree.locomotion(model).play_action(&clip);
            }
        }
    }
}
