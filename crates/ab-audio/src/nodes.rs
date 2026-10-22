use crate::class;
use crate::components::{Attenuation, AudioListener3D, AudioPlayer};
use ab_scene::{NodeHandle, SceneTree, Transform};

/// Audio node constructors on [`SceneTree`] (`use ab_audio::AudioNodes;`).
pub trait AudioNodes {
    /// Creates a detached `AudioStreamPlayer` (no position: music, UI sounds).
    fn create_audio_stream_player(&mut self, name: &str, player: AudioPlayer) -> NodeHandle;

    /// Creates a detached `AudioStreamPlayer3D`; a player without attenuation gets the
    /// default one.
    fn create_audio_stream_player3d(&mut self, name: &str, player: AudioPlayer) -> NodeHandle;

    /// Creates a detached `AudioListener3D`, usually added under the camera.
    fn create_audio_listener3d(&mut self, name: &str) -> NodeHandle;
}

impl AudioNodes for SceneTree {
    fn create_audio_stream_player(&mut self, name: &str, player: AudioPlayer) -> NodeHandle {
        let player = AudioPlayer {
            positional: None,
            ..player
        };
        self.spawn_node(name, class::AUDIO_STREAM_PLAYER, (player,))
    }

    fn create_audio_stream_player3d(&mut self, name: &str, mut player: AudioPlayer) -> NodeHandle {
        player.positional.get_or_insert_with(Attenuation::default);
        self.spawn_node(
            name,
            class::AUDIO_STREAM_PLAYER_3D,
            (Transform::default(), player),
        )
    }

    fn create_audio_listener3d(&mut self, name: &str) -> NodeHandle {
        self.spawn_node(
            name,
            class::AUDIO_LISTENER_3D,
            (Transform::default(), AudioListener3D::default()),
        )
    }
}
