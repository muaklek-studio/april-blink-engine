//! Locomotion: picks and blends a character's clips from how it moves.
//!
//! Put a [`Locomotion`] next to a model's [`AnimationPlayer`] and tell it, every frame, how
//! fast the character goes, whether it stands on the ground, climbs, or how fast it rises
//! or falls. It does the rest:
//!
//! * on the ground, blends the ground clips by speed (idle → walk → run), in step so the
//!   feet do not slide, and speeds the fastest clip up beyond its own speed;
//! * in the air, plays the jump clip while rising and the fall clip while falling;
//! * climbing, plays the climb clip at the climbing speed (paused when still, backwards
//!   going down);
//! * [`Locomotion::play_action`] plays a one-shot clip over all that (an attack, a reload, a
//!   dodge) and returns to moving when it ends;
//! * every change crossfades.
//!
//! ```
//! use ab_animation::Locomotion;
//! let mut loco = Locomotion::humanoid();
//! loco.speed = 3.0; // m/s, from the character's velocity
//! loco.grounded = true;
//! ```

use crate::player::{AnimationLibrary, AnimationPlayer};
use ab_ecs::World;

/// What a [`Locomotion`] is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocomotionState {
    #[default]
    Ground,
    Jump,
    Fall,
    Climb,
    Action,
}

/// Drives the [`AnimationPlayer`] on the same node from movement (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Locomotion {
    /// Ground clips and the speed (m/s) each was made for, slowest first: idle at 0, walk,
    /// run...
    pub ground: Vec<(String, f32)>,
    /// Played while rising (`vertical_speed > 0` in the air).
    pub jump: Option<String>,
    /// Played while falling.
    pub fall: Option<String>,
    pub climb: Option<String>,
    /// Climbing speed (m/s) the climb clip was made for.
    pub climb_speed: f32,
    /// Crossfade time in seconds.
    pub fade: f32,

    /// Horizontal speed in m/s (set every frame).
    pub speed: f32,
    /// On the ground (set every frame).
    pub grounded: bool,
    /// Up (+) or down (-) speed in m/s: rising or falling in the air, along the wall when
    /// climbing (set every frame).
    pub vertical_speed: f32,
    /// On a wall or ladder (set every frame).
    pub climbing: bool,

    action: Option<String>,
    action_started: bool,
    state: LocomotionState,
    /// Index of the ground clip playing as the main one.
    ground_clip: Option<usize>,
}

impl Default for Locomotion {
    fn default() -> Self {
        Self {
            ground: Vec::new(),
            jump: None,
            fall: None,
            climb: None,
            climb_speed: 1.0,
            fade: 0.2,
            speed: 0.0,
            grounded: true,
            vertical_speed: 0.0,
            climbing: false,
            action: None,
            action_started: false,
            state: LocomotionState::Ground,
            ground_clip: None,
        }
    }
}

impl Locomotion {
    /// The usual humanoid set, named as in the engine's sample mannequin and most packs:
    /// `Idle`, `Walk` (1.5 m/s), `Run` (4.2 m/s), `Jump`, `Fall`, `Climb` (0.8 m/s).
    pub fn humanoid() -> Self {
        Self {
            ground: vec![
                ("Idle".into(), 0.0),
                ("Walk".into(), 1.5),
                ("Run".into(), 4.2),
            ],
            jump: Some("Jump".into()),
            fall: Some("Fall".into()),
            climb: Some("Climb".into()),
            climb_speed: 0.8,
            ..Self::default()
        }
    }

    /// Plays `clip` once over the movement (attack, reload, dodge...), then goes back.
    pub fn play_action(&mut self, clip: &str) {
        self.action = Some(clip.to_owned());
        self.action_started = false;
    }

    /// Ends the action early.
    pub fn stop_action(&mut self) {
        self.action = None;
    }

    pub fn is_action_playing(&self) -> bool {
        self.action.is_some()
    }

    pub fn state(&self) -> LocomotionState {
        self.state
    }

    /// The ground clips to play at `speed`: `(main, blended, weight)`.
    fn ground_mix(&self, speed: f32) -> Option<(usize, Option<usize>, f32)> {
        let last = self.ground.len().checked_sub(1)?;
        let speed = speed.max(0.0);
        let i = self
            .ground
            .windows(2)
            .position(|w| speed < w[1].1)
            .unwrap_or(last);
        if i == last {
            return Some((last, None, 0.0));
        }
        let (low, high) = (self.ground[i].1, self.ground[i + 1].1);
        let weight = ((speed - low) / (high - low).max(1e-4)).clamp(0.0, 1.0);
        Some((i, Some(i + 1), weight))
    }
}

/// Updates the [`AnimationPlayer`] of every node with a [`Locomotion`]. The engine runs it in
/// `Update`, before the animations.
pub fn locomotion_system(world: &mut World) {
    let Some(library) = world.resource::<AnimationLibrary>() else {
        return;
    };
    let (Some(mut locos), Some(mut players)) = (
        world.storage_mut::<Locomotion>(),
        world.storage_mut::<AnimationPlayer>(),
    ) else {
        return;
    };
    for (entity, loco) in locos.iter_mut() {
        if let Some(player) = players.get_mut(entity) {
            drive(loco, player, &library);
        }
    }
}

fn drive(loco: &mut Locomotion, player: &mut AnimationPlayer, library: &AnimationLibrary) {
    let fade = loco.fade.max(0.0);
    // An action runs until its clip ends.
    if let Some(name) = loco.action.clone() {
        match player.clip_named(&name) {
            None => loco.action = None,
            Some(clip) if !loco.action_started => {
                player.crossfade(clip, fade);
                player.looping = false;
                player.speed = 1.0;
                loco.action_started = true;
                loco.state = LocomotionState::Action;
                loco.ground_clip = None;
                return;
            }
            Some(clip) if player.current == Some(clip) && player.playing => return,
            Some(_) => loco.action = None,
        }
    }
    let state = if loco.climbing && loco.climb.is_some() {
        LocomotionState::Climb
    } else if !loco.grounded && loco.vertical_speed > 0.5 && loco.jump.is_some() {
        LocomotionState::Jump
    } else if !loco.grounded && loco.fall.is_some() {
        // A jump clip keeps playing over the top of the jump.
        if loco.state == LocomotionState::Jump && player.playing {
            LocomotionState::Jump
        } else {
            LocomotionState::Fall
        }
    } else {
        LocomotionState::Ground
    };
    let entered = state != loco.state;
    loco.state = state;
    let named = |name: &Option<String>| name.as_deref().and_then(|n| player.clip_named(n));
    match state {
        LocomotionState::Jump | LocomotionState::Fall => {
            loco.ground_clip = None;
            let (clip, looping) = if state == LocomotionState::Jump {
                (named(&loco.jump), false)
            } else {
                (named(&loco.fall), true)
            };
            if let Some(clip) = clip.filter(|_| entered) {
                player.crossfade(clip, fade);
                player.looping = looping;
                player.speed = 1.0;
            }
        }
        LocomotionState::Climb => {
            loco.ground_clip = None;
            if let Some(clip) = named(&loco.climb) {
                if entered {
                    player.crossfade(clip, fade);
                    player.looping = true;
                }
                player.playing = true;
                player.speed = loco.vertical_speed / loco.climb_speed.max(0.01);
            }
        }
        LocomotionState::Ground => {
            let Some((main, blended, weight)) = loco.ground_mix(loco.speed) else {
                return;
            };
            let Some(main_clip) = player.clip_named(&loco.ground[main].0) else {
                return;
            };
            let blend_clip = blended.and_then(|i| player.clip_named(&loco.ground[i].0));
            if entered || loco.ground_clip.is_none() {
                player.crossfade(main_clip, fade);
                player.looping = true;
            } else if loco.ground_clip != Some(main) {
                // Between two ground clips: keep the phase, so the steps carry on.
                let duration = |c| library.get(c).map_or(0.0, |c| c.duration);
                let phase = player
                    .current
                    .map(|c| player.time / duration(c).max(1e-4))
                    .unwrap_or(0.0);
                player.current = Some(main_clip);
                player.time = phase * duration(main_clip);
                player.playing = true;
                player.looping = true;
            }
            loco.ground_clip = Some(main);
            player.blend = blend_clip.map(|c| (c, weight));
            // The blend matches speeds between clips; past the fastest, it speeds up.
            let clip_speed = match blended {
                Some(i) => {
                    let (low, high) = (loco.ground[main].1, loco.ground[i].1);
                    low + (high - low) * weight
                }
                None => loco.ground[main].1,
            };
            player.speed = if clip_speed > 0.01 && blended.is_none() {
                (loco.speed / clip_speed).clamp(0.25, 3.0)
            } else {
                1.0
            };
        }
        LocomotionState::Action => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{AnimationClip, Channel, Interpolation, Keyframes};
    use crate::player::apply_animations;
    use ab_core::Vec3;
    use ab_scene::SceneTree;

    /// A player knowing clips that each move one bone to a different height.
    fn setup() -> (SceneTree, ab_scene::NodeHandle) {
        let mut tree = SceneTree::new();
        let bone = tree.create_node3d("Bone");
        tree.add_child(tree.root(), bone).unwrap();
        let mut library = AnimationLibrary::new();
        let mut player = AnimationPlayer::new(vec![bone]);
        for (i, name) in ["Idle", "Walk", "Run", "Jump", "Fall", "Climb", "Attack"]
            .into_iter()
            .enumerate()
        {
            let y = i as f32;
            let clip = library.add(AnimationClip::new(
                name,
                vec![Channel {
                    target: 0,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    keyframes: Keyframes::Translation(vec![
                        Vec3::new(0.0, y, 0.0),
                        Vec3::new(1.0, y, 0.0),
                    ]),
                }],
            ));
            player.clips.push((name.into(), clip));
        }
        tree.world_mut().insert_resource(library);
        tree.insert_component(bone, player).unwrap();
        tree.insert_component(
            bone,
            Locomotion {
                fade: 0.0,
                ..Locomotion::humanoid()
            },
        )
        .unwrap();
        (tree, bone)
    }

    fn frame(tree: &mut SceneTree, dt: f32) {
        locomotion_system(tree.world_mut());
        apply_animations(tree.world(), dt);
    }

    fn height(tree: &SceneTree, bone: ab_scene::NodeHandle) -> f32 {
        tree.component::<ab_scene::Transform>(bone)
            .unwrap()
            .local
            .translation
            .y
    }

    #[track_caller]
    fn assert_near(value: f32, expected: f32, what: &str) {
        assert!(
            (value - expected).abs() < 1e-3,
            "{what}: {value} vs {expected}"
        );
    }

    fn loco(tree: &SceneTree, bone: ab_scene::NodeHandle) -> std::cell::RefMut<'_, Locomotion> {
        tree.world().get_mut::<Locomotion>(bone.entity()).unwrap()
    }

    #[test]
    fn blends_ground_clips_by_speed() {
        let (mut tree, bone) = setup();
        frame(&mut tree, 0.1);
        assert_near(height(&tree, bone), 0.0, "idle");
        loco(&tree, bone).speed = 0.75; // half way to walking
        frame(&mut tree, 0.1);
        assert!(
            (height(&tree, bone) - 0.5).abs() < 1e-4,
            "{}",
            height(&tree, bone)
        );
        loco(&tree, bone).speed = 4.2;
        frame(&mut tree, 0.1);
        assert!((height(&tree, bone) - 2.0).abs() < 1e-4, "run");
        // Faster than the run clip: it plays faster.
        loco(&tree, bone).speed = 8.4;
        frame(&mut tree, 0.1);
        let player = tree.component::<AnimationPlayer>(bone).unwrap();
        assert!((player.speed - 2.0).abs() < 1e-4);
    }

    #[test]
    fn walking_into_running_keeps_the_step_phase() {
        let (mut tree, bone) = setup();
        loco(&tree, bone).speed = 1.5;
        frame(&mut tree, 0.0);
        frame(&mut tree, 0.4);
        let before = tree.component::<AnimationPlayer>(bone).unwrap().time;
        loco(&tree, bone).speed = 4.2;
        frame(&mut tree, 0.0);
        let after = tree.component::<AnimationPlayer>(bone).unwrap().time;
        assert!((before - after).abs() < 1e-4, "{before} vs {after}");
    }

    #[test]
    fn air_climb_and_actions() {
        let (mut tree, bone) = setup();
        {
            let mut l = loco(&tree, bone);
            l.grounded = false;
            l.vertical_speed = 4.0;
        }
        frame(&mut tree, 0.1);
        assert_near(height(&tree, bone), 3.0, "jumping");
        assert_eq!(loco(&tree, bone).state(), LocomotionState::Jump);
        // The jump clip ends, still in the air and falling: fall.
        loco(&tree, bone).vertical_speed = -2.0;
        frame(&mut tree, 1.5);
        frame(&mut tree, 0.1);
        assert_near(height(&tree, bone), 4.0, "falling");
        {
            let mut l = loco(&tree, bone);
            l.climbing = true;
            l.vertical_speed = 0.0;
        }
        frame(&mut tree, 0.1);
        assert_near(height(&tree, bone), 5.0, "climbing");
        let paused = tree.component::<AnimationPlayer>(bone).unwrap().speed;
        assert_eq!(paused, 0.0, "holding still on the wall");
        {
            let mut l = loco(&tree, bone);
            l.climbing = false;
            l.grounded = true;
            l.play_action("Attack");
        }
        frame(&mut tree, 0.1);
        assert_near(height(&tree, bone), 6.0, "attacking");
        assert!(loco(&tree, bone).is_action_playing());
        frame(&mut tree, 1.2);
        frame(&mut tree, 0.1);
        assert!(!loco(&tree, bone).is_action_playing());
        assert_near(height(&tree, bone), 0.0, "back to idle");
    }

    #[test]
    fn crossfades_blend_the_old_pose_out() {
        let (mut tree, bone) = setup();
        loco(&tree, bone).fade = 0.4;
        frame(&mut tree, 0.0);
        frame(&mut tree, 0.1);
        loco(&tree, bone).play_action("Attack");
        frame(&mut tree, 0.0);
        frame(&mut tree, 0.2); // half way through the fade
        let y = height(&tree, bone);
        assert!(y > 0.5 && y < 5.5, "between idle (0) and attack (6): {y}");
        frame(&mut tree, 0.3);
        assert_near(height(&tree, bone), 6.0, "height");
    }
}
