//! Ready-made nodes for the Create menu: plain nodes, primitives, physics objects with
//! their mesh and shape, lights, cameras, sounds and every particle effect preset.

use crate::state::Editor;
use crate::theme;
use ab_engine::core::Vec3;
use ab_engine::effects::ParticleSettings;
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
    section(ui, "Node", BASIC);
    section(ui, "Mesh", MESHES);
    section(ui, "Light", LIGHTS);
    section(ui, "Camera", CAMERAS);
    section(ui, "Physics", PHYSICS);
    ui.menu_button("Ragdoll", |ui| {
        if entry(ui, "Ragdoll", "Humanoid (falls on Play)") {
            chosen = Some(ragdoll(true));
            ui.close();
        }
        if entry(ui, "Ragdoll", "Humanoid (stands until hit)") {
            chosen = Some(ragdoll(false));
            ui.close();
        }
        if entry(ui, "Ragdoll", "Empty Ragdoll") {
            chosen = Some(SceneFile {
                environment: None,
                nodes: vec![ab_engine::ragdoll::ragdoll_node("Ragdoll", true)],
            });
            ui.close();
        }
    });
    ui.menu_button("Effect", |ui| {
        for preset in ParticleSettings::PRESETS {
            let mut label: String = preset[..1].to_uppercase();
            label.push_str(&preset[1..]);
            if entry(ui, "Particles3D", &label) {
                chosen = Some(effect(preset));
                ui.close();
            }
        }
    });
    if entry(ui, "AudioStreamPlayer3D", "Sound") {
        chosen = Some(sound(editor));
        ui.close();
    }
    if let Some(scene) = chosen {
        editor.add_scene(scene, Some(at));
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
        let all: Vec<&Template> = [BASIC, MESHES, LIGHTS, CAMERAS, PHYSICS]
            .iter()
            .flat_map(|group| group.iter())
            .collect();
        let mut scenes: Vec<SceneFile> = all.iter().map(|t| t.scene()).collect();
        scenes.extend(ParticleSettings::PRESETS.iter().map(|p| effect(p)));
        scenes.push(sound(&editor));
        scenes.push(ragdoll(true));
        scenes.push(ragdoll(false));
        for scene in scenes {
            let name = scene.nodes[0].name.clone();
            editor.selection = None;
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
