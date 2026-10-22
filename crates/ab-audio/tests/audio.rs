//! Scene-level audio tests on the silent backend, which advances playback with frame time.

use ab_audio::{
    audio_system, Attenuation, Audio, AudioListener3D, AudioNodes, AudioPlayer, AudioStream,
    SoundOptions,
};
use ab_core::{Quat, Time, Vec3};
use ab_scene::{NodeHandle, SceneTree};

const DT: f32 = 1.0 / 60.0;

fn new_tree() -> SceneTree {
    let mut tree = SceneTree::new();
    let mut time = Time::new(DT);
    time.begin_frame(DT);
    tree.world_mut().insert_resource(time);
    tree.world_mut().insert_resource(Audio::silent());
    tree
}

/// One frame: end-of-frame update (transforms, frees), then audio.
fn frame(tree: &mut SceneTree) {
    tree.process(DT);
    audio_system(tree.world_mut());
}

/// Frames until `node` stops playing (or `limit`), returning the count.
fn frames_until_stopped(tree: &mut SceneTree, node: NodeHandle, limit: usize) -> usize {
    for n in 1..=limit {
        frame(tree);
        if !player(tree, node).is_playing() {
            return n;
        }
    }
    limit
}

fn player(tree: &SceneTree, node: NodeHandle) -> AudioPlayer {
    tree.component::<AudioPlayer>(node).unwrap().clone()
}

/// Half a second of silence.
fn half_second() -> AudioStream {
    AudioStream::from_samples(8000, &[0.0; 4000])
}

fn finished(tree: &SceneTree) -> Vec<NodeHandle> {
    tree.world()
        .resource::<Audio>()
        .unwrap()
        .finished()
        .to_vec()
}

#[test]
fn sound_plays_for_its_duration_then_reports_finished() {
    let mut tree = new_tree();
    let node = tree.create_audio_stream_player("Sfx", AudioPlayer::new(half_second()));
    tree.add_child(tree.root(), node).unwrap();
    frame(&mut tree);
    assert!(!player(&tree, node).is_playing());

    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    assert!(
        player(&tree, node).is_playing(),
        "play() is visible at once"
    );
    let mut seen_finished = 0;
    let mut frames = 0;
    while player(&tree, node).is_playing() && frames < 120 {
        frame(&mut tree);
        frames += 1;
        seen_finished += finished(&tree).iter().filter(|&&n| n == node).count();
    }
    // 0.5 s at 60 fps, give or take a mixing chunk.
    assert!((29..=32).contains(&frames), "stopped after {frames} frames");
    assert_eq!(seen_finished, 1);
    frame(&mut tree);
    assert!(finished(&tree).is_empty());
}

#[test]
fn pitch_scale_changes_speed() {
    let mut tree = new_tree();
    let node = tree.create_audio_stream_player(
        "Fast",
        AudioPlayer::new(half_second()).with_pitch_scale(2.0),
    );
    tree.add_child(tree.root(), node).unwrap();
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    let frames = frames_until_stopped(&mut tree, node, 120);
    assert!((14..=17).contains(&frames), "stopped after {frames} frames");
}

#[test]
fn looping_plays_until_stopped() {
    let mut tree = new_tree();
    let node = tree
        .create_audio_stream_player("Music", AudioPlayer::new(half_second()).with_looping(true));
    tree.add_child(tree.root(), node).unwrap();
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    for _ in 0..120 {
        frame(&mut tree);
    }
    assert!(player(&tree, node).is_playing(), "still looping after 2 s");
    assert!(finished(&tree).is_empty());

    // Volume and pitch can change while playing.
    {
        let mut music = tree.component_mut::<AudioPlayer>(node).unwrap();
        music.volume_db = -12.0;
        music.pitch_scale = 0.5;
    }
    frame(&mut tree);
    assert!(player(&tree, node).is_playing());

    tree.component_mut::<AudioPlayer>(node).unwrap().stop();
    assert!(!player(&tree, node).is_playing());
    frame(&mut tree);
    frame(&mut tree);
    assert!(!player(&tree, node).is_playing());
    assert!(finished(&tree).is_empty(), "stop() is not a natural finish");
}

#[test]
fn play_restarts_from_the_beginning() {
    let mut tree = new_tree();
    let node = tree.create_audio_stream_player("Sfx", AudioPlayer::new(half_second()));
    tree.add_child(tree.root(), node).unwrap();
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    for _ in 0..20 {
        frame(&mut tree);
    }
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    let frames = frames_until_stopped(&mut tree, node, 120);
    assert!(frames >= 29, "restarted, so a full 0.5 s again: {frames}");
}

#[test]
fn autoplay_follows_the_tree() {
    let mut tree = new_tree();
    let holder = tree.create_node("Holder");
    tree.add_child(tree.root(), holder).unwrap();
    let node = tree.create_audio_stream_player3d(
        "Ambience",
        AudioPlayer::new(half_second())
            .with_looping(true)
            .with_autoplay(true),
    );
    assert!(
        player(&tree, node).positional.is_some(),
        "3D players are positional"
    );
    frame(&mut tree);
    assert!(!player(&tree, node).is_playing(), "not in the tree yet");

    tree.add_child(holder, node).unwrap();
    frame(&mut tree);
    assert!(player(&tree, node).is_playing());

    tree.remove_child(holder, node).unwrap();
    frame(&mut tree);
    assert!(
        !player(&tree, node).is_playing(),
        "leaving the tree stops it"
    );
    // play() outside the tree is ignored, as in Godot.
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    frame(&mut tree);
    assert!(!player(&tree, node).is_playing());

    tree.add_child(holder, node).unwrap();
    frame(&mut tree);
    assert!(
        player(&tree, node).is_playing(),
        "autoplay again on re-entry"
    );

    // Switching between 2D and 3D, or changing attenuation, keeps it playing.
    tree.component_mut::<AudioPlayer>(node).unwrap().positional = None;
    frame(&mut tree);
    frame(&mut tree);
    assert!(player(&tree, node).is_playing());
    tree.component_mut::<AudioPlayer>(node).unwrap().positional = Some(Attenuation {
        min_distance: 2.0,
        max_distance: 10.0,
    });
    frame(&mut tree);
    frame(&mut tree);
    assert!(player(&tree, node).is_playing());

    tree.queue_free(holder).unwrap();
    frame(&mut tree);
    assert!(!tree.is_valid(node));
}

#[test]
fn player_without_stream_does_nothing() {
    let mut tree = new_tree();
    let mut silent = AudioPlayer::new(half_second());
    silent.stream = None;
    let node = tree.create_audio_stream_player("Empty", silent);
    tree.add_child(tree.root(), node).unwrap();
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    frame(&mut tree);
    assert!(!player(&tree, node).is_playing());
    assert!(finished(&tree).is_empty());
}

#[test]
fn listener_follows_the_current_listener_node() {
    let mut tree = new_tree();
    frame(&mut tree);
    let audio = |tree: &SceneTree| {
        tree.world()
            .resource::<Audio>()
            .unwrap()
            .listener_transform()
    };
    assert_eq!(audio(&tree), (Vec3::ZERO, Quat::IDENTITY));

    let camera = tree.create_node3d("Camera");
    tree.set_position(camera, Vec3::new(1.0, 2.0, 3.0)).unwrap();
    tree.set_rotation(camera, Quat::from_rotation_y(1.0))
        .unwrap();
    tree.add_child(tree.root(), camera).unwrap();
    let ears = tree.create_audio_listener3d("Ears");
    tree.add_child(camera, ears).unwrap();
    let other = tree.create_audio_listener3d("Other");
    tree.component_mut::<AudioListener3D>(other)
        .unwrap()
        .current = false;
    tree.set_position(other, Vec3::splat(50.0)).unwrap();
    tree.add_child(tree.root(), other).unwrap();

    frame(&mut tree);
    let (position, rotation) = audio(&tree);
    assert!(position.abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-5));
    assert!(rotation.abs_diff_eq(Quat::from_rotation_y(1.0), 1e-5));

    // The current listener moves with its parent.
    tree.translate(camera, Vec3::new(0.0, 0.0, -1.0)).unwrap();
    frame(&mut tree);
    assert!(audio(&tree).0.abs_diff_eq(
        Vec3::new(1.0, 2.0, 3.0) + Quat::from_rotation_y(1.0) * Vec3::NEG_Z,
        1e-5
    ));

    // Without a current listener the first one is used.
    tree.component_mut::<AudioListener3D>(ears).unwrap().current = false;
    tree.queue_free(camera).unwrap();
    frame(&mut tree);
    assert!(audio(&tree).0.abs_diff_eq(Vec3::splat(50.0), 1e-5));
}

#[test]
fn one_shots_and_master_volume() {
    let tree = new_tree();
    let world = tree.world();
    let mut audio = world.resource_mut::<Audio>().unwrap();
    assert!(audio.is_silent());
    let stream = half_second();
    audio.play(&stream, SoundOptions::default());
    for i in 0..600 {
        audio.play_at(
            &stream,
            Vec3::new(i as f32, 0.0, 0.0),
            SoundOptions {
                pitch_scale: 1.5,
                ..Default::default()
            },
        );
        if i % 50 == 0 {
            audio.update(world, 0.5);
        }
    }
    audio.set_master_volume_db(-6.0);
    assert_eq!(audio.master_volume_db(), -6.0);
    audio.update(world, 1.0);
}

#[test]
fn finished_signal_reaches_scripts() {
    use ab_scene::{NodeScript, Signal};

    #[derive(Default)]
    struct Done(bool);
    impl NodeScript for Done {
        fn on_signal(&mut self, _: &mut SceneTree, _: NodeHandle, signal: &Signal) {
            self.0 |= signal.is(AudioPlayer::FINISHED);
        }
    }

    let mut tree = new_tree();
    let node = tree.create_audio_stream_player("Sfx", AudioPlayer::new(half_second()));
    tree.add_child(tree.root(), node).unwrap();
    let done = tree.create_node("Done");
    tree.set_script(done, Done::default()).unwrap();
    tree.add_child(tree.root(), done).unwrap();
    tree.connect(node, AudioPlayer::FINISHED, done);
    tree.component_mut::<AudioPlayer>(node).unwrap().play();
    frames_until_stopped(&mut tree, node, 120);
    frame(&mut tree); // process delivers the queued signal
    assert!(tree.script::<Done>(done).unwrap().0);
}
