use crate::clip::{AnimationClip, Sample};
use ab_core::{jobs, Time};
use ab_ecs::{Entity, World};
use ab_scene::{NodeHandle, Transform};

/// Reference to a clip in the [`AnimationLibrary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClipHandle(u32);

impl ClipHandle {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// ECS resource owning every loaded [`AnimationClip`].
#[derive(Debug)]
pub struct AnimationLibrary {
    id: u64,
    clips: Vec<AnimationClip>,
}

impl Default for AnimationLibrary {
    fn default() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            clips: Vec::new(),
        }
    }
}

impl AnimationLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unique per instance: clip handles are only meaningful in the library that issued
    /// them, so caches of handles (e.g. the asset server's) remember which one that was.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn add(&mut self, clip: AnimationClip) -> ClipHandle {
        let handle = ClipHandle(self.clips.len() as u32);
        self.clips.push(clip);
        handle
    }

    pub fn get(&self, handle: ClipHandle) -> Option<&AnimationClip> {
        self.clips.get(handle.index())
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

/// Plays clips on a set of target nodes, Godot's `AnimationPlayer`.
///
/// Channels address targets by index into `targets`, so one clip can drive any node set with
/// the same layout (e.g. every instance of an imported model).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationPlayer {
    /// Clips this player knows by name (e.g. all animations of an imported glTF).
    pub clips: Vec<(String, ClipHandle)>,
    pub targets: Vec<NodeHandle>,
    pub current: Option<ClipHandle>,
    /// Playback position in seconds.
    pub time: f32,
    /// Playback rate; negative plays backwards.
    pub speed: f32,
    pub looping: bool,
    pub playing: bool,
    /// A second clip mixed into the current one at a weight (0..1), kept in step with it
    /// (same phase): walk and run blended by speed. See [`Locomotion`](crate::Locomotion).
    pub blend: Option<(ClipHandle, f32)>,
    /// The clip being faded out under the current one ([`AnimationPlayer::crossfade`]).
    pub fade: Option<Fade>,
}

/// A clip fading out while the next one fades in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub clip: ClipHandle,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    pub blend: Option<(ClipHandle, f32)>,
    /// Seconds the fade takes, and has run.
    pub duration: f32,
    pub elapsed: f32,
}

impl AnimationPlayer {
    pub fn new(targets: Vec<NodeHandle>) -> Self {
        Self {
            clips: Vec::new(),
            targets,
            current: None,
            time: 0.0,
            speed: 1.0,
            looping: true,
            playing: false,
            blend: None,
            fade: None,
        }
    }

    /// The clip registered under `name`.
    pub fn clip_named(&self, name: &str) -> Option<ClipHandle> {
        self.clips.iter().find(|(n, _)| n == name).map(|&(_, c)| c)
    }

    /// The name of the clip playing.
    pub fn current_name(&self) -> Option<&str> {
        let current = self.current?;
        self.clips
            .iter()
            .find(|(_, c)| *c == current)
            .map(|(n, _)| n.as_str())
    }

    /// Starts `clip` from the beginning, blending from the current pose over `seconds`
    /// instead of snapping to it. Already playing `clip`: nothing changes.
    pub fn crossfade(&mut self, clip: ClipHandle, seconds: f32) {
        if self.current == Some(clip) && self.playing {
            return;
        }
        match self.current.filter(|_| seconds > 0.0) {
            Some(from) => {
                self.fade = Some(Fade {
                    clip: from,
                    time: self.time,
                    speed: if self.playing { self.speed } else { 0.0 },
                    looping: self.looping,
                    blend: self.blend,
                    duration: seconds,
                    elapsed: 0.0,
                });
            }
            None => self.fade = None,
        }
        self.blend = None;
        self.play(clip);
    }

    /// [`AnimationPlayer::crossfade`] to the clip registered under `name`. Returns `false`
    /// if there is none.
    pub fn crossfade_named(&mut self, name: &str, seconds: f32) -> bool {
        match self.clip_named(name) {
            Some(clip) => {
                self.crossfade(clip, seconds);
                true
            }
            None => false,
        }
    }

    /// Starts `clip` from the beginning.
    pub fn play(&mut self, clip: ClipHandle) {
        self.current = Some(clip);
        self.time = 0.0;
        self.playing = true;
    }

    /// Starts the clip registered under `name`. Returns `false` if there is none.
    pub fn play_named(&mut self, name: &str) -> bool {
        match self.clips.iter().find(|(n, _)| n == name) {
            Some(&(_, clip)) => {
                self.play(clip);
                true
            }
            None => false,
        }
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn resume(&mut self) {
        self.playing = self.current.is_some();
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.time = 0.0;
    }

    pub fn clip_names(&self) -> impl Iterator<Item = &str> {
        self.clips.iter().map(|(name, _)| name.as_str())
    }

    /// Advances `time` by `delta` (scaled by `speed`), wrapping or clamping at the ends.
    fn advance(&mut self, delta: f32, duration: f32) {
        if !self.playing {
            return;
        }
        (self.time, self.playing) = step(self.time, delta * self.speed, duration, self.looping);
    }
}

/// Moves a playback position by `by` seconds: `(time, still playing)`.
fn step(time: f32, by: f32, duration: f32, looping: bool) -> (f32, bool) {
    let time = time + by;
    if duration <= 0.0 {
        (0.0, true)
    } else if looping {
        (time.rem_euclid(duration), true)
    } else if time >= duration || time < 0.0 {
        (time.clamp(0.0, duration), false)
    } else {
        (time, true)
    }
}

/// Mixes two samples of the same property: `t` = 0 is `a`, 1 is `b`.
fn mix(a: Sample, b: Sample, t: f32) -> Sample {
    match (a, b) {
        (Sample::Translation(x), Sample::Translation(y)) => Sample::Translation(x.lerp(y, t)),
        (Sample::Scale(x), Sample::Scale(y)) => Sample::Scale(x.lerp(y, t)),
        (Sample::Rotation(x), Sample::Rotation(y)) => Sample::Rotation(x.slerp(y, t)),
        _ => b,
    }
}

fn property(sample: &Sample) -> u8 {
    match sample {
        Sample::Translation(_) => 0,
        Sample::Rotation(_) => 1,
        Sample::Scale(_) => 2,
    }
}

/// Blends `other` into `pose` by `weight`: shared properties are mixed, the others taken
/// from whichever side outweighs.
fn blend_into(pose: &mut Vec<(Entity, Sample)>, other: &[(Entity, Sample)], weight: f32) {
    if weight <= 0.0 {
        return;
    }
    let start = pose.len();
    for &(entity, sample) in other {
        let kind = property(&sample);
        match pose[..start]
            .iter_mut()
            .find(|(e, s)| *e == entity && property(s) == kind)
        {
            Some((_, mine)) => *mine = mix(*mine, sample, weight),
            None if weight >= 0.5 => pose.push((entity, sample)),
            None => {}
        }
    }
}

/// Samples `clip` at `time`, with `blend`'s clip mixed in at the same phase.
fn sample_clips(
    library: &AnimationLibrary,
    targets: &[NodeHandle],
    clip: ClipHandle,
    time: f32,
    blend: Option<(ClipHandle, f32)>,
    out: &mut Vec<(Entity, Sample)>,
) {
    let Some(main) = library.get(clip) else {
        return;
    };
    let sample = |clip: &AnimationClip, time: f32, out: &mut Vec<(Entity, Sample)>| {
        out.extend(clip.channels.iter().filter_map(|channel| {
            let target = targets.get(channel.target)?;
            Some((target.entity(), channel.sample(time)))
        }));
    };
    sample(main, time, out);
    if let Some((other, weight)) = blend.filter(|&(c, w)| c != clip && w > 0.0) {
        let Some(other) = library.get(other) else {
            return;
        };
        let phase = if main.duration > 0.0 {
            time / main.duration
        } else {
            0.0
        };
        let mut mixed = Vec::with_capacity(other.channels.len());
        sample(other, phase * other.duration, &mut mixed);
        blend_into(out, &mixed, weight.min(1.0));
    }
}

/// Writes the pose of the player on `entity` at its current time into its targets, without
/// advancing it (editors scrubbing a timeline).
pub fn apply_pose(world: &World, entity: Entity) {
    let (Some(library), Some(player)) = (
        world.resource::<AnimationLibrary>(),
        world.get::<AnimationPlayer>(entity).map(|p| p.clone()),
    ) else {
        return;
    };
    let Some(clip) = player.current else {
        return;
    };
    let mut pose = Vec::new();
    sample_clips(
        &library,
        &player.targets,
        clip,
        player.time,
        player.blend,
        &mut pose,
    );
    let Some(mut transforms) = world.storage_mut::<Transform>() else {
        return;
    };
    for (target, sample) in pose {
        let Some(transform) = transforms.get_mut(target) else {
            continue;
        };
        match sample {
            Sample::Translation(v) => transform.local.translation = v,
            Sample::Rotation(q) => transform.local.rotation = q,
            Sample::Scale(v) => transform.local.scale = v,
        }
        transform.dirty = true;
    }
}

/// Advances every [`AnimationPlayer`] by the frame delta (from the [`Time`] resource) and
/// writes the sampled poses into the targets' local transforms.
pub fn animation_system(world: &mut World) {
    let delta = world.resource::<Time>().map_or(0.0, |t| t.delta);
    apply_animations(world, delta);
}

/// Sampled poses of every player, kept between frames so sampling does not allocate.
/// Inserted by [`crate::register`]; without it [`apply_animations`] uses temporary buffers.
#[derive(Debug, Default)]
pub struct AnimationPoses(Vec<Vec<(Entity, Sample)>>);

/// Below this many channels to sample in total, players are sampled on the calling thread.
const PARALLEL_CHANNELS: usize = 256;

/// [`animation_system`] with an explicit delta (tools, tests, scrubbing).
///
/// Players advance and sample their clips in parallel (on the [`ab_core::jobs`] workers
/// for busy scenes); the poses are then written to the targets in player order, so when two
/// players drive the same node the later one wins, as in a serial loop.
pub fn apply_animations(world: &World, delta: f32) {
    let Some(library) = world.resource::<AnimationLibrary>() else {
        return;
    };
    let Some(mut players) = world.storage_mut::<AnimationPlayer>() else {
        return;
    };
    let mut stored = world.resource_mut::<AnimationPoses>();
    let mut local = Vec::new();
    let poses = match stored.as_deref_mut() {
        Some(AnimationPoses(poses)) => poses,
        None => &mut local,
    };
    let players = players.values_mut();
    poses.resize_with(players.len(), Vec::new);
    let library = &*library;
    let channels: usize = players
        .iter()
        .filter(|p| p.playing)
        .filter_map(|p| p.current.and_then(|c| library.get(c)))
        .map(|clip| clip.channels.len())
        .sum();
    let min_per_job = if channels < PARALLEL_CHANNELS {
        usize::MAX
    } else {
        1
    };
    let mut work: Vec<_> = players.iter_mut().zip(poses.iter_mut()).collect();
    jobs::par_for_each_mut(&mut work, min_per_job, |(player, pose)| {
        pose.clear();
        let Some(clip) = player.current.and_then(|c| library.get(c)) else {
            return;
        };
        let was_playing = player.playing;
        player.advance(delta, clip.duration);
        // The clip fading out keeps running under the new one.
        let fade = player.fade.as_mut().map(|fade| {
            let duration = library.get(fade.clip).map_or(0.0, |c| c.duration);
            fade.time = step(fade.time, delta * fade.speed, duration, fade.looping).0;
            fade.elapsed += delta;
            *fade
        });
        if !was_playing && fade.is_none() {
            return;
        }
        let current = player.current.expect("checked above");
        sample_clips(
            library,
            &player.targets,
            current,
            player.time,
            player.blend,
            pose,
        );
        if let Some(fade) = fade {
            let mut from = Vec::with_capacity(pose.len());
            sample_clips(
                library,
                &player.targets,
                fade.clip,
                fade.time,
                fade.blend,
                &mut from,
            );
            let t = (fade.elapsed / fade.duration.max(1e-4)).clamp(0.0, 1.0);
            // Smoothstep: no kink at either end.
            let t = t * t * (3.0 - 2.0 * t);
            blend_into(&mut from, pose, t);
            **pose = from;
            if fade.elapsed >= fade.duration {
                player.fade = None;
            }
        }
    });
    drop(work);

    let Some(mut transforms) = world.storage_mut::<Transform>() else {
        return;
    };
    for (entity, sample) in poses.iter().flatten() {
        let Some(transform) = transforms.get_mut(*entity) else {
            continue;
        };
        match *sample {
            Sample::Translation(v) => transform.local.translation = v,
            Sample::Rotation(q) => transform.local.rotation = q,
            Sample::Scale(v) => transform.local.scale = v,
        }
        transform.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Channel, Interpolation, Keyframes};
    use ab_core::{Quat, Vec3};
    use ab_scene::SceneTree;

    fn setup() -> (SceneTree, NodeHandle, ClipHandle) {
        let mut tree = SceneTree::new();
        let bone = tree.create_node3d("Bone");
        tree.add_child(tree.root(), bone).unwrap();
        let mut library = AnimationLibrary::new();
        let clip = library.add(AnimationClip::new(
            "Raise",
            vec![
                Channel {
                    target: 0,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 2.0],
                    keyframes: Keyframes::Translation(vec![Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)]),
                },
                Channel {
                    target: 0,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 2.0],
                    keyframes: Keyframes::Rotation(vec![
                        Quat::IDENTITY,
                        Quat::from_rotation_z(1.0),
                    ]),
                },
            ],
        ));
        tree.world_mut().insert_resource(library);
        (tree, bone, clip)
    }

    fn player(tree: &SceneTree, node: NodeHandle) -> AnimationPlayer {
        tree.component::<AnimationPlayer>(node).unwrap().clone()
    }

    #[test]
    fn plays_clip_onto_targets() {
        let (mut tree, bone, clip) = setup();
        let mut p = AnimationPlayer::new(vec![bone]);
        p.clips.push(("Raise".into(), clip));
        assert!(p.play_named("Raise"));
        assert!(!p.play_named("Missing"));
        tree.insert_component(bone, p).unwrap();

        apply_animations(tree.world(), 0.5);
        assert!(tree
            .position(bone)
            .unwrap()
            .abs_diff_eq(Vec3::new(0.0, 0.5, 0.0), 1e-5));
        assert!(tree
            .rotation(bone)
            .unwrap()
            .abs_diff_eq(Quat::from_rotation_z(0.25), 1e-5));
        tree.update_transforms();
        assert!(tree
            .global_position(bone)
            .unwrap()
            .abs_diff_eq(Vec3::new(0.0, 0.5, 0.0), 1e-5));
    }

    #[test]
    fn looping_wraps_and_one_shot_stops_at_the_end() {
        let (mut tree, bone, clip) = setup();
        let mut p = AnimationPlayer::new(vec![bone]);
        p.play(clip);
        tree.insert_component(bone, p).unwrap();
        apply_animations(tree.world(), 2.5);
        assert!((player(&tree, bone).time - 0.5).abs() < 1e-5);

        tree.component_mut::<AnimationPlayer>(bone).unwrap().looping = false;
        apply_animations(tree.world(), 3.0);
        let p = player(&tree, bone);
        assert!(!p.playing);
        assert_eq!(p.time, 2.0);
        assert!(tree
            .position(bone)
            .unwrap()
            .abs_diff_eq(Vec3::new(0.0, 2.0, 0.0), 1e-5));
    }

    #[test]
    fn paused_players_do_not_move_targets() {
        let (mut tree, bone, clip) = setup();
        let mut p = AnimationPlayer::new(vec![bone]);
        p.play(clip);
        p.pause();
        tree.insert_component(bone, p).unwrap();
        tree.set_position(bone, Vec3::splat(9.0)).unwrap();
        apply_animations(tree.world(), 1.0);
        assert_eq!(tree.position(bone).unwrap(), Vec3::splat(9.0));
    }

    #[test]
    fn many_players_sample_in_parallel_like_serially() {
        let (mut tree, bone, clip) = setup();
        let mut nodes = vec![bone];
        for i in 0..PARALLEL_CHANNELS {
            let extra = tree.create_node3d(&format!("Extra{i}"));
            tree.add_child(tree.root(), extra).unwrap();
            nodes.push(extra);
        }
        for &node in &nodes {
            let mut p = AnimationPlayer::new(vec![node]);
            p.play(clip);
            tree.insert_component(node, p).unwrap();
        }
        tree.world_mut().insert_resource(AnimationPoses::default());
        apply_animations(tree.world(), 0.5);
        for &node in &nodes {
            let position = tree.position(node).unwrap();
            assert!((position.y - 0.5).abs() < 1e-5, "{position:?}");
            assert!((player(&tree, node).time - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn system_uses_time_resource() {
        let (mut tree, bone, clip) = setup();
        let mut p = AnimationPlayer::new(vec![bone]);
        p.play(clip);
        tree.insert_component(bone, p).unwrap();
        let mut time = Time::default();
        time.begin_frame(0.2);
        tree.world_mut().insert_resource(time);
        animation_system(tree.world_mut());
        assert!(tree
            .position(bone)
            .unwrap()
            .abs_diff_eq(Vec3::new(0.0, 0.2, 0.0), 1e-5));
    }
}
