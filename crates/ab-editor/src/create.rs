//! Ready-made nodes for the Create menu: plain nodes, primitives, physics objects with
//! their mesh and shape, lights, cameras, sounds, every particle effect preset, and UI
//! controls and screens.

use crate::i18n::{t as tr, tf};
use crate::state::Editor;
use crate::theme;
use ab_engine::core::Vec3;
use ab_engine::effects::ParticleSettings;
use ab_engine::scene::NodeHandle;
use ab_engine::scene_file::SceneFile;
use eframe::egui;

/// One menu entry: a label, the class whose icon it shows, and the nodes it creates.
pub struct Template {
    pub label: &'static str,
    pub class: &'static str,
    pub toml: &'static str,
}

const fn t(label: &'static str, class: &'static str, toml: &'static str) -> Template {
    Template { label, class, toml }
}

pub const BASIC: &[Template] = &[
    t(
        "Node",
        "Node",
        "[[node]]\nname = \"Node\"\ntype = \"Node\"\n",
    ),
    t(
        "Node3D",
        "Node3D",
        "[[node]]\nname = \"Node3D\"\ntype = \"Node3D\"\n",
    ),
    t(
        "Timer",
        "Timer",
        "[[node]]\nname = \"Timer\"\ntype = \"Timer\"\ntimer = { wait_time = 1.0 }\n",
    ),
];

pub const MESHES: &[Template] = &[
    t(
        "Cube",
        "MeshInstance3D",
        "[[node]]\nname = \"Cube\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { color = \"#c8ccd4\", roughness = 0.6 }\n",
    ),
    t(
        "Sphere",
        "MeshInstance3D",
        "[[node]]\nname = \"Sphere\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"sphere\" }\nmaterial = { color = \"#c8ccd4\", roughness = 0.4 }\n",
    ),
    t(
        "Plane",
        "MeshInstance3D",
        "[[node]]\nname = \"Plane\"\ntype = \"MeshInstance3D\"\nscale = [10.0, 1.0, 10.0]\nmesh = { primitive = \"plane\" }\nmaterial = { color = \"#7a7f88\", roughness = 0.9 }\n",
    ),
];

/// Paths and what they build (pick a piece for the fence in the Inspector).
pub const PATHS: &[Template] = &[
    t(
        "Path",
        "Path3D",
        "[[node]]\nname = \"Path\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.0, 0.0], [4.0, 0.0, 2.0], [8.0, 0.0, 0.0]] }\n",
    ),
    t(
        "Road",
        "Path3D",
        "[[node]]\nname = \"Road\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.02, 0.0], [10.0, 0.02, 3.0], [20.0, 0.02, 0.0]], mode = \"ribbon\", width = 4.0, collide = true }\nmaterial = { color = \"#3a3b3e\", roughness = 0.9 }\n",
    ),
    t(
        "River",
        "Path3D",
        "[[node]]\nname = \"River\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.01, 0.0], [8.0, 0.01, 6.0], [16.0, 0.01, 2.0], [24.0, 0.01, 8.0]], mode = \"ribbon\", width = 6.0 }\nmaterial = { color = \"#2f6f9fcc\", roughness = 0.05, metallic = 0.2 }\n",
    ),
    t(
        "Pipe",
        "Path3D",
        "[[node]]\nname = \"Pipe\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.5, 0.0], [4.0, 0.5, 0.0], [4.0, 3.0, 2.0]], mode = \"tube\", radius = 0.2, sides = 16, collide = true }\nmaterial = { color = \"#8a9099\", roughness = 0.35, metallic = 1.0 }\n",
    ),
    t(
        "Cable",
        "Path3D",
        "[[node]]\nname = \"Cable\"\ntype = \"Path3D\"\npath = { points = [[0.0, 4.0, 0.0], [10.0, 4.0, 0.0]], mode = \"tube\", radius = 0.025, sides = 6, sag = 0.8 }\nmaterial = { color = \"#1a1a1a\" }\n",
    ),
    t(
        "Fence / row of pieces",
        "Path3D",
        "[[node]]\nname = \"Fence\"\ntype = \"Path3D\"\npath = { points = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]], mode = \"instances\", spacing = 2.0 }\n",
    ),
];

/// Procedural generators (their settings are in the Inspector).
pub const PROCEDURAL: &[Template] = &[
    t(
        "Scatter (trees, rocks…)",
        "Generator",
        "[[node]]\nname = \"Scatter\"\ntype = \"Generator\"\ngenerator = { kind = \"scatter\", seed = 1 }\n",
    ),
    t(
        "Terrain (hills, mountains)",
        "Generator",
        "[[node]]\nname = \"Terrain\"\ntype = \"Generator\"\ngenerator = { kind = \"terrain\", seed = 1 }\n",
    ),
    t(
        "Dungeon",
        "Generator",
        "[[node]]\nname = \"Dungeon\"\ntype = \"Generator\"\ngenerator = { kind = \"dungeon\", seed = 1 }\n",
    ),
    t(
        "Building",
        "Generator",
        "[[node]]\nname = \"Building\"\ntype = \"Generator\"\ngenerator = { kind = \"building\", seed = 1 }\n",
    ),
    t(
        "Town",
        "Generator",
        "[[node]]\nname = \"Town\"\ntype = \"Generator\"\ngenerator = { kind = \"town\", seed = 1 }\n",
    ),
];

pub const LIGHTS: &[Template] = &[
    t(
        "Directional Light",
        "DirectionalLight3D",
        "[[node]]\nname = \"Sun\"\ntype = \"DirectionalLight3D\"\nrotation = [-50.0, -35.0, 0.0]\nlight = { energy = 2.5, shadows = true }\n",
    ),
    t(
        "Omni Light",
        "OmniLight3D",
        "[[node]]\nname = \"OmniLight\"\ntype = \"OmniLight3D\"\nlight = { color = \"#ffd9a8\", energy = 4.0, range = 8.0 }\n",
    ),
    t(
        "Spot Light",
        "SpotLight3D",
        "[[node]]\nname = \"SpotLight\"\ntype = \"SpotLight3D\"\nrotation = [-60.0, 0.0, 0.0]\nlight = { energy = 8.0, range = 12.0, angle = 30.0, shadows = true }\n",
    ),
];

pub const CAMERAS: &[Template] = &[
    t(
        "Camera",
        "Camera3D",
        "[[node]]\nname = \"Camera\"\ntype = \"Camera3D\"\ncamera = { fov = 70.0 }\n",
    ),
    t(
        "Shaking Camera",
        "CameraShake",
        "[[node]]\nname = \"Shake\"\ntype = \"CameraShake\"\n\n[[node]]\nname = \"Camera\"\nparent = \".\"\ntype = \"Camera3D\"\ncamera = { fov = 70.0 }\n",
    ),
    t(
        "Audio Listener",
        "AudioListener3D",
        "[[node]]\nname = \"Listener\"\ntype = \"AudioListener3D\"\n",
    ),
    t(
        "HLOD Group",
        "HLOD",
        "[[node]]\nname = \"HLOD\"\ntype = \"HLOD\"\nhlod = { distance = 80.0, detail = 0.3 }\n",
    ),
    t(
        "Post-Process Volume",
        "PostVolume",
        "[[node]]\nname = \"PostVolume\"\ntype = \"PostVolume\"\npost_volume = { size = [10.0, 6.0, 10.0], blend = 3.0, saturation = 0.6, vignette = 0.35 }\n",
    ),
];

pub const PHYSICS: &[Template] = &[
    t(
        "Static Box",
        "StaticBody3D",
        "[[node]]\nname = \"Wall\"\ntype = \"StaticBody3D\"\n\n[[node]]\nname = \"Mesh\"\nparent = \".\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { color = \"#8a8f99\", roughness = 0.8 }\n\n[[node]]\nname = \"Shape\"\nparent = \".\"\ntype = \"CollisionShape3D\"\nshape = { box = [1.0, 1.0, 1.0] }\n",
    ),
    t(
        "Rigid Box",
        "RigidBody3D",
        "[[node]]\nname = \"Crate\"\ntype = \"RigidBody3D\"\nposition = [0.0, 2.0, 0.0]\nbody = { mass = 1.0 }\n\n[[node]]\nname = \"Mesh\"\nparent = \".\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"cube\" }\nmaterial = { color = \"#b07a4a\", roughness = 0.7 }\n\n[[node]]\nname = \"Shape\"\nparent = \".\"\ntype = \"CollisionShape3D\"\nshape = { box = [1.0, 1.0, 1.0] }\n",
    ),
    t(
        "Rigid Ball",
        "RigidBody3D",
        "[[node]]\nname = \"Ball\"\ntype = \"RigidBody3D\"\nposition = [0.0, 2.0, 0.0]\nbody = { mass = 0.5, bounce = 0.5 }\n\n[[node]]\nname = \"Mesh\"\nparent = \".\"\ntype = \"MeshInstance3D\"\nmesh = { primitive = \"sphere\" }\nmaterial = { color = \"#d04a3c\", roughness = 0.35 }\n\n[[node]]\nname = \"Shape\"\nparent = \".\"\ntype = \"CollisionShape3D\"\nshape = { sphere = 0.5 }\n",
    ),
    t(
        "Character",
        "CharacterBody3D",
        "[[node]]\nname = \"Character\"\ntype = \"CharacterBody3D\"\nposition = [0.0, 1.0, 0.0]\n\n[[node]]\nname = \"Shape\"\nparent = \".\"\ntype = \"CollisionShape3D\"\nshape = { capsule = [0.4, 1.8] }\n",
    ),
    t(
        "Trigger Area",
        "Area3D",
        "[[node]]\nname = \"Area\"\ntype = \"Area3D\"\n\n[[node]]\nname = \"Shape\"\nparent = \".\"\ntype = \"CollisionShape3D\"\nshape = { box = [2.0, 2.0, 2.0] }\n",
    ),
    t(
        "Collision Shape",
        "CollisionShape3D",
        "[[node]]\nname = \"Shape\"\ntype = \"CollisionShape3D\"\nshape = { box = [1.0, 1.0, 1.0] }\n",
    ),
    t(
        "Pin Joint",
        "Joint3D",
        "[[node]]\nname = \"PinJoint\"\ntype = \"Joint3D\"\njoint = { kind = \"pin\", body_a = \"\", body_b = \"\" }\n",
    ),
    t(
        "Hinge Joint",
        "Joint3D",
        "[[node]]\nname = \"HingeJoint\"\ntype = \"Joint3D\"\njoint = { kind = \"hinge\", body_a = \"\", body_b = \"\", limits = [-90.0, 90.0] }\n",
    ),
    t(
        "Cone-Twist Joint",
        "Joint3D",
        "[[node]]\nname = \"ConeJoint\"\ntype = \"Joint3D\"\njoint = { kind = \"cone\", body_a = \"\", body_b = \"\", swing = 45.0, twist = 30.0 }\n",
    ),
];

pub const UI: &[Template] = &[
    t(
        "Canvas Layer",
        "CanvasLayer",
        "[[node]]\nname = \"UI\"\ntype = \"CanvasLayer\"\n",
    ),
    t(
        "Control",
        "Control",
        "[[node]]\nname = \"Control\"\ntype = \"Control\"\ncontrol = { preset = \"full_rect\" }\n",
    ),
    t(
        "Panel",
        "Panel",
        "[[node]]\nname = \"Panel\"\ntype = \"Panel\"\ncontrol = { preset = \"center\", size = [400.0, 300.0] }\n",
    ),
    t(
        "Label",
        "Label",
        "[[node]]\nname = \"Label\"\ntype = \"Label\"\ncontrol = { preset = \"center\", size = [300.0, 40.0] }\nlabel = { text = \"Label\", align = \"center\" }\n",
    ),
    t(
        "Button",
        "Button",
        "[[node]]\nname = \"Button\"\ntype = \"Button\"\ncontrol = { preset = \"center\", size = [220.0, 56.0] }\nbutton = { text = \"Button\" }\n",
    ),
    t(
        "Progress Bar",
        "ProgressBar",
        "[[node]]\nname = \"ProgressBar\"\ntype = \"ProgressBar\"\ncontrol = { preset = \"center\", size = [360.0, 28.0] }\nprogress = { value = 60.0 }\n",
    ),
    t(
        "Vertical Box",
        "VBoxContainer",
        "[[node]]\nname = \"VBox\"\ntype = \"VBoxContainer\"\ncontrol = { preset = \"center\", size = [300.0, 240.0] }\n",
    ),
    t(
        "Horizontal Box",
        "HBoxContainer",
        "[[node]]\nname = \"HBox\"\ntype = \"HBoxContainer\"\ncontrol = { preset = \"center\", size = [480.0, 64.0] }\n",
    ),
];

/// Whole screens to start from.
pub const UI_SCREENS: &[Template] = &[
    t(
        "HUD (health, score, crosshair)",
        "Control",
        r##"[[node]]
name = "Hud"
type = "Control"
control = { preset = "full_rect" }

[[node]]
name = "Health"
parent = "."
type = "ProgressBar"
control = { preset = "top_left", size = [360.0, 28.0], margin = 32.0 }
progress = { value = 100.0, fill_color = "#d23c3c" }

[[node]]
name = "Score"
parent = "."
type = "Label"
control = { preset = "top_right", size = [320.0, 44.0], margin = 32.0 }
label = { text = "Score: 0", size = 32.0, align = "right", outline = 2.0 }

[[node]]
name = "Crosshair"
parent = "."
type = "Label"
control = { preset = "center", size = [40.0, 40.0] }
label = { text = "+", size = 36.0, align = "center", valign = "center", outline = 2.0 }
"##,
    ),
    t(
        "Pause menu",
        "Panel",
        r##"[[node]]
name = "PauseMenu"
type = "Control"
control = { preset = "full_rect" }

[[node]]
name = "Dim"
parent = "."
type = "Panel"
control = { preset = "full_rect" }
panel = { color = "#00000099", corner_radius = 0.0 }

[[node]]
name = "Window"
parent = "."
type = "Panel"
control = { preset = "center", size = [440.0, 380.0] }
panel = { color = "#141a24f0", corner_radius = 12.0, border_width = 1.0, border_color = "#ffffff30" }

[[node]]
name = "Items"
parent = "Window"
type = "VBoxContainer"
control = { preset = "full_rect", margin = 32.0 }
box = { separation = 14.0 }

[[node]]
name = "Title"
parent = "Window/Items"
type = "Label"
control = { min_size = [0.0, 64.0] }
label = { text = "Paused", size = 44.0, align = "center" }

[[node]]
name = "Resume"
parent = "Window/Items"
type = "Button"
control = { min_size = [0.0, 56.0] }
button = { text = "Resume" }

[[node]]
name = "Settings"
parent = "Window/Items"
type = "Button"
control = { min_size = [0.0, 56.0] }
button = { text = "Settings" }

[[node]]
name = "Quit"
parent = "Window/Items"
type = "Button"
control = { min_size = [0.0, 56.0] }
button = { text = "Quit", color = "#8a2f2f" }
"##,
    ),
    t(
        "Dialogue box",
        "Panel",
        r##"[[node]]
name = "Dialogue"
type = "Panel"
control = { preset = "bottom_wide", size = [0.0, 220.0], margin = 40.0 }
panel = { color = "#0c1018e6", corner_radius = 10.0 }

[[node]]
name = "Speaker"
parent = "."
type = "Label"
control = { anchors = [0.0, 0.0, 1.0, 0.0], offsets = [28.0, 20.0, -28.0, 64.0] }
label = { text = "Speaker", size = 30.0, color = "#ffcc66" }

[[node]]
name = "Text"
parent = "."
type = "Label"
control = { anchors = [0.0, 0.0, 1.0, 1.0], offsets = [28.0, 72.0, -28.0, -20.0] }
label = { text = "What they say goes here.", size = 26.0, autowrap = true }
"##,
    ),
];

/// An image showing the project's first picture (empty texture path when it has none).
pub fn image(editor: &Editor) -> Option<SceneFile> {
    let texture = editor
        .assets
        .of_kind(crate::project::AssetKind::Image)
        .next()?
        .path
        .clone();
    let mut scene = SceneFile::from_toml(
        "[[node]]\nname = \"Image\"\ntype = \"TextureRect\"\ncontrol = { preset = \"center\", size = [256.0, 256.0] }\nimage = { texture = \"\", stretch = \"keep_aspect\" }\n",
    )
    .expect("the image template parses");
    if let Some(image) = scene.nodes[0].image.as_mut() {
        image.texture = texture;
    }
    Some(scene)
}

fn is_ui(editor: &Editor, node: NodeHandle) -> bool {
    let tree = editor.tree();
    tree.component::<ab_engine::ui::Control>(node).is_some()
        || tree.class_name(node) == Some(ab_engine::ui::class::CANVAS_LAYER)
}

/// Controls go under a UI node: the selected one, else the scene's first canvas layer
/// (selected for that), else a new `UI` canvas layer wrapping them.
pub fn place_ui(editor: &mut Editor, mut scene: SceneFile) -> SceneFile {
    let class = scene.nodes[0].kind.as_deref().unwrap_or_default();
    if class == ab_engine::ui::class::CANVAS_LAYER {
        return scene;
    }
    let selected_ui = editor
        .selection
        .is_some_and(|n| is_ui(editor, n) && editor.is_editable(n) && !editor.is_instance(n));
    if selected_ui {
        return scene;
    }
    let tree = editor.tree();
    let layer = tree
        .children(editor.scene_root)
        .into_iter()
        .find(|&c| tree.class_name(c) == Some(ab_engine::ui::class::CANVAS_LAYER));
    if let Some(layer) = layer {
        editor.select(Some(layer));
        return scene;
    }
    let root = scene.nodes[0].name.clone();
    for node in &mut scene.nodes {
        node.parent = Some(match node.parent.take().as_deref() {
            None => ".".to_owned(),
            Some(".") => root.clone(),
            Some(path) => format!("{root}/{path}"),
        });
    }
    let mut layer = UI[0].scene();
    layer.nodes.append(&mut scene.nodes);
    layer
}

/// The humanoid ragdoll prefab: limp from the start, or standing frozen until activated.
pub fn ragdoll(active: bool) -> SceneFile {
    ab_engine::ragdoll::humanoid_ragdoll(&ab_engine::ragdoll::HumanoidRagdoll {
        name: if active { "Ragdoll" } else { "Dummy" }.into(),
        active,
        ..Default::default()
    })
}

/// Particle effect node with a preset.
pub fn effect(preset: &str) -> SceneFile {
    let mut name: String = preset[..1].to_uppercase();
    name.push_str(&preset[1..]);
    SceneFile::from_toml(&format!(
        "[[node]]\nname = \"{name}\"\ntype = \"Particles3D\"\nparticles = {{ preset = \"{preset}\" }}\n"
    ))
    .expect("effect templates parse")
}

/// An effect with layers (a campfire: fire, smoke, embers) to edit in the Effect editor and
/// save under effects/.
pub fn layered_effect() -> SceneFile {
    SceneFile::from_toml(
        "[[node]]\nname = \"Effect\"\ntype = \"Node3D\"\n\n\
         [[node]]\nname = \"Fire\"\nparent = \".\"\ntype = \"Particles3D\"\nparticles = { preset = \"fire\" }\n\n\
         [[node]]\nname = \"Smoke\"\nparent = \".\"\ntype = \"Particles3D\"\nposition = [0.0, 0.6, 0.0]\nparticles = { preset = \"smoke\" }\n\n\
         [[node]]\nname = \"Embers\"\nparent = \".\"\ntype = \"Particles3D\"\nparticles = { preset = \"embers\" }\n",
    )
    .expect("the layered effect template parses")
}

/// A positional sound playing the first sound file of the project, if any.
pub fn sound(editor: &Editor) -> SceneFile {
    let stream = editor
        .assets
        .of_kind(crate::project::AssetKind::Audio)
        .next()
        .map(|a| a.path.clone())
        .unwrap_or_default();
    let mut scene = SceneFile::from_toml(
        "[[node]]\nname = \"Sound\"\ntype = \"AudioStreamPlayer3D\"\naudio = { stream = \"\", autoplay = true, looping = true }\n",
    )
    .expect("the sound template parses");
    if let Some(audio) = scene.nodes[0].audio.as_mut() {
        audio.stream = stream;
    }
    scene
}

impl Template {
    pub fn scene(&self) -> SceneFile {
        SceneFile::from_toml(self.toml).expect("templates parse")
    }
}

fn entry(ui: &mut egui::Ui, class: &str, label: &str) -> bool {
    ui.button(egui::RichText::new(format!(
        "{}  {label}",
        theme::class_icon(class)
    )))
    .clicked()
}

/// The Create menu body. New nodes go at `at` (the view's focus point).
pub fn menu(ui: &mut egui::Ui, editor: &mut Editor, at: Vec3) {
    ui.set_min_width(200.0);
    let mut chosen: Option<SceneFile> = None;
    let mut section = |ui: &mut egui::Ui, title: &str, templates: &[Template]| {
        ui.menu_button(title, |ui| {
            for template in templates {
                if entry(ui, template.class, template.label) {
                    chosen = Some(template.scene());
                    ui.close();
                }
            }
        });
    };
    section(ui, &tr("create.node"), BASIC);
    section(ui, &tr("create.mesh"), MESHES);
    section(ui, &tr("create.path"), PATHS);
    section(ui, &tr("create.procedural"), PROCEDURAL);
    section(ui, &tr("create.light"), LIGHTS);
    section(ui, &tr("create.camera"), CAMERAS);
    section(ui, &tr("create.physics"), PHYSICS);
    if entry(ui, "AnimationPlayer", &tr("create.animation_player")) {
        chosen = Some(SceneFile {
            environment: None,
            nodes: vec![crate::panels::animation::new_player()],
        });
        ui.close();
    }
    if entry(ui, ab_engine::cutscene::CUTSCENE, &tr("create.cutscene")) {
        chosen = Some(SceneFile {
            environment: None,
            nodes: vec![ab_engine::scene_file::NodeDesc {
                name: "Cutscene".into(),
                kind: Some(ab_engine::cutscene::CUTSCENE.into()),
                cutscene: Some(Default::default()),
                ..Default::default()
            }],
        });
        ui.close();
    }
    // A rigged character selected: a ragdoll made from its skeleton.
    let character = editor
        .selection
        .filter(|&n| editor.is_instance(n))
        .filter(|&n| {
            let tree = editor.tree();
            tree.subtree(n)
                .into_iter()
                .any(|m| tree.component::<ab_engine::rendering::Skin>(m).is_some())
        })
        .and_then(|n| editor.tree().name(n).map(|name| name.to_owned()));
    ui.menu_button(tr("create.ragdoll"), |ui| {
        if let Some(name) = &character {
            if entry(ui, "Ragdoll", &tf("create.ragdoll_for", &[("name", name)])) {
                let mut node = ab_engine::ragdoll::ragdoll_node("Ragdoll", false);
                if let Some(desc) = node.ragdoll.as_mut() {
                    desc.skeleton = Some(format!("../{name}"));
                }
                chosen = Some(SceneFile {
                    environment: None,
                    nodes: vec![node],
                });
                ui.close();
            }
            ui.separator();
        }
        if entry(ui, "Ragdoll", &tr("create.ragdoll_falls")) {
            chosen = Some(ragdoll(true));
            ui.close();
        }
        if entry(ui, "Ragdoll", &tr("create.ragdoll_stands")) {
            chosen = Some(ragdoll(false));
            ui.close();
        }
        if entry(ui, "Ragdoll", &tr("create.ragdoll_empty")) {
            chosen = Some(SceneFile {
                environment: None,
                nodes: vec![ab_engine::ragdoll::ragdoll_node("Ragdoll", true)],
            });
            ui.close();
        }
    });
    ui.menu_button(tr("create.effect"), |ui| {
        if entry(ui, "Node3D", &tr("create.layered_effect")) {
            chosen = Some(layered_effect());
            ui.close();
        }
        ui.separator();
        for preset in ParticleSettings::PRESETS {
            let mut label: String = preset[..1].to_uppercase();
            label.push_str(&preset[1..].replace('_', " "));
            if entry(ui, "Particles3D", &label) {
                chosen = Some(effect(preset));
                ui.close();
            }
        }
    });
    if entry(ui, "AudioStreamPlayer3D", &tr("create.sound")) {
        chosen = Some(sound(editor));
        ui.close();
    }
    // A UI node: its controls are made in the UI Editor, which opens on it.
    if entry(ui, "CanvasLayer", &tr("create.ui")) {
        ui.close();
        // At the top of the scene: an interface is not part of a 3D object.
        editor.select(None);
        editor.add_scene(UI[0].scene(), None);
        if let Some(layer) = editor.selection {
            editor.open_request = Some(layer);
        }
    }
    if let Some(scene) = chosen {
        editor.add_scene(scene, Some(at));
        // An AnimationPlayer, a cutscene or an effect opens in its editor, ready to work on.
        if let Some(node) = editor
            .selection
            .filter(|&n| crate::features::Feature::of(editor, n).is_some())
        {
            editor.open_request = Some(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_parses_and_builds() {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(project, None, ab_engine::audio::AudioOutput::Silent);
        let all: Vec<&Template> = [
            BASIC, MESHES, PATHS, PROCEDURAL, LIGHTS, CAMERAS, PHYSICS, UI, UI_SCREENS,
        ]
        .iter()
        .flat_map(|group| group.iter())
        .collect();
        let mut scenes: Vec<SceneFile> = all.iter().map(|t| t.scene()).collect();
        scenes.extend(ParticleSettings::PRESETS.iter().map(|p| effect(p)));
        scenes.push(layered_effect());
        scenes.push(sound(&editor));
        scenes.push(ragdoll(true));
        scenes.push(ragdoll(false));
        scenes.extend(image(&editor));
        for scene in scenes {
            let name = scene.nodes[0].name.clone();
            editor.select(None);
            editor.add_scene(scene, Some(Vec3::ZERO));
            assert_eq!(
                editor.status.as_ref().map(|s| s.error),
                Some(false),
                "{name}: {:?}",
                editor.status.as_ref().map(|s| &s.text)
            );
        }
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    fn editor() -> Editor {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        Editor::new(project, None, ab_engine::audio::AudioOutput::Silent)
    }

    #[test]
    fn controls_get_a_canvas_layer_then_share_it() {
        let mut editor = editor();
        editor.select(None);
        let scene = place_ui(&mut editor, UI[3].scene());
        assert_eq!(scene.nodes[0].kind.as_deref(), Some("CanvasLayer"));
        editor.add_scene(scene, None);
        let layer = editor.selection.unwrap();
        assert_eq!(editor.tree().children(layer).len(), 1);

        // The next control joins that layer, even with a 3D node selected.
        editor.select(Some(editor.scene_root));
        let screen = place_ui(&mut editor, UI_SCREENS[1].scene());
        editor.add_scene(screen, None);
        let menu = editor.selection.unwrap();
        assert_eq!(editor.tree().parent(menu), Some(layer));
        let window = editor.tree().get_node(menu, "Window/Items/Quit");
        assert!(window.is_some());

        // Inside a selected control: straight in.
        let label = place_ui(&mut editor, UI[3].scene());
        assert_eq!(label.nodes.len(), 1);
    }
}
