//! Property tests of the scene file format and save slots: whatever the input, parsing never
//! panics, and what is saved reads back the same.

use ab_engine::save::SaveStore;
use ab_engine::scene_file::{EnvironmentDesc, NodeDesc, SceneFile, ShapeDesc};
use proptest::prelude::*;

/// Text with the characters that trouble TOML writers: quotes, backslashes, newlines,
/// control characters, Thai.
fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            4 => prop::char::range('a', 'z'),
            1 => prop::sample::select(vec!['"', '\'', '\\', '\n', '\t', '\u{7f}', ' ', '.', '/', '=', '#', 'ก', 'ข']),
            1 => any::<char>(),
        ],
        0..12,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn finite() -> impl Strategy<Value = f32> {
    prop_oneof![
        -1.0e6f32..1.0e6,
        prop::sample::select(vec![0.0, -0.0, 1.0e-30, f32::MAX, f32::MIN_POSITIVE]),
    ]
}

fn vec3() -> impl Strategy<Value = [f32; 3]> {
    [finite(), finite(), finite()]
}

fn shape() -> impl Strategy<Value = ShapeDesc> {
    prop_oneof![
        vec3().prop_map(ShapeDesc::Box),
        finite().prop_map(ShapeDesc::Sphere),
        [finite(), finite()].prop_map(ShapeDesc::Capsule),
    ]
}

prop_compose! {
    fn node()(
        name in text(),
        parent in prop::option::of(text()),
        kind in prop::option::of(text()),
        position in prop::option::of(vec3()),
        rotation in prop::option::of(vec3()),
        scale in prop::option::of(vec3()),
        visible in prop::option::of(any::<bool>()),
        groups in prop::collection::vec(text(), 0..3),
        script in prop::option::of(text()),
        shape in prop::option::of(shape()),
    ) -> NodeDesc {
        NodeDesc {
            name,
            parent,
            kind,
            position,
            rotation,
            scale,
            visible,
            groups,
            script,
            shape,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn scene()(
        nodes in prop::collection::vec(node(), 0..6),
        fog in prop::option::of(0.0f32..1.0),
    ) -> SceneFile {
        SceneFile {
            environment: fog.map(|fog_density| EnvironmentDesc {
                fog_density: Some(fog_density),
                ..Default::default()
            }),
            nodes,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn scenes_read_back_as_saved(scene in scene()) {
        let text = scene.to_toml();
        let parsed = SceneFile::from_toml(&text);
        prop_assert!(parsed.is_ok(), "{:?}\n{text}", parsed.err());
        prop_assert_eq!(parsed.unwrap(), scene);
    }

    #[test]
    fn any_text_parses_or_errors_without_panicking(text in "\\PC{0,200}") {
        let _ = SceneFile::from_toml(&text);
    }

    /// Damaged files: a valid scene with a slice of it removed or repeated.
    #[test]
    fn damaged_scenes_never_panic(scene in scene(), cut in any::<prop::sample::Index>(), len in 0usize..40) {
        let text = scene.to_toml();
        let start = cut.index(text.len() + 1);
        let end = (start + len).min(text.len());
        if let (Some(head), Some(tail), Some(middle)) = (text.get(..start), text.get(end..), text.get(start..end)) {
            let _ = SceneFile::from_toml(&format!("{head}{tail}"));
            let _ = SceneFile::from_toml(&format!("{head}{middle}{middle}{tail}"));
        }
    }

    #[test]
    fn save_slots_never_panic_on_damaged_files(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let dir = std::env::temp_dir().join(format!("ab-save-props-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("slot.json"), &bytes).unwrap();
        let saves = SaveStore::in_dir(&dir);
        let _ = saves.load::<serde_json::Value>("slot");
        let _ = saves.load_versioned::<serde_json::Value>("slot", 3, |_, _| Ok(()));
    }
}
