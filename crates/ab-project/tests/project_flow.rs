//! The whole life of a project, for real (minutes: builds the player and the scripts):
//! `cargo test -p ab-project --test project_flow -- --ignored`.
//!
//! A new project is created outside the engine, its scripts are built as a separate cargo
//! workspace against the SDK, and the game is exported with the player and run from an empty
//! folder: the editor never needs the engine's sources for any of it.

use ab_project::export::{export, Game, Options};
use ab_project::{create, BuildState, NewProject, Profile, Project, ScriptBuild};

#[test]
#[ignore]
fn a_new_project_builds_and_exports_with_the_player() {
    let root = std::env::temp_dir().join(format!("ab-project-flow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let project = create(&NewProject {
        title: "Flow Check".into(),
        location: root.clone(),
        new_folder: true,
    })
    .unwrap();

    // A script, as the editor would write it, attached in the main scene.
    std::fs::write(
        project.root.join("src/scripts/spinner.rs"),
        "use ab_script::prelude::*;\n\n\
         #[derive(Debug, Default, Serialize, Deserialize)]\n\
         pub struct Spinner {\n    turned: f32,\n}\n\n\
         impl Script for Spinner {\n    serde_state!();\n\n    \
         fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {\n        \
         self.turned += delta;\n        let _ = tree.rotate(node, Vec3::Y, delta);\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        project.root.join("src/scripts/mod.rs"),
        "pub mod spinner;\n\npub fn register(registry: &mut ab_script::Registry) {\n    \
         registry.register::<spinner::Spinner>(\"Spinner\");\n}\n",
    )
    .unwrap();
    let scene = project.assets.join("scenes/main.abscene");
    let mut text = std::fs::read_to_string(&scene).unwrap();
    text.push_str(
        "\n[[node]]\nname = \"Spinner\"\nparent = \".\"\ntype = \"Node3D\"\nscript = \"Spinner\"\n",
    );
    std::fs::write(&scene, text).unwrap();

    // The editor's build.
    let mut build = ScriptBuild::start(&project, Profile::Debug).unwrap();
    assert_eq!(*build.wait(), BuildState::Succeeded);
    assert!(project.editor_library().unwrap().is_file());

    // Export with the player, verified by running it headless from an empty folder.
    let out = root.join("dist");
    let mut log = Vec::new();
    let result = export(
        &Game::Project(Box::new(Project::open(&project.root).unwrap())),
        &Options::new(&out),
        &mut |line: &str| log.push(line.to_owned()),
    );
    let log = log.join("\n");
    let exported = result.unwrap_or_else(|err| panic!("{err:#}\n{log}"));
    assert_eq!(exported.len(), 1);
    let folder = &exported[0].folder;
    let platform = ab_project::Platform::host();
    // On macOS the game and its files are inside the app bundle.
    let folder = if platform == ab_project::Platform::MacOs {
        folder.join("Flow Check.app/Contents/MacOS")
    } else {
        folder.clone()
    };
    for file in [
        platform.executable("flow-check"),
        "assets/index.abm".to_owned(),
        platform.shipped_library("flow-check"),
    ] {
        assert!(folder.join(&file).is_file(), "{file} missing\n{log}");
    }
    assert!(
        log.contains("the exported game runs from its pack outside the source tree"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
