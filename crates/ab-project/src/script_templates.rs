//! The starter scripts offered when making a new script. Their sources live in the
//! `ab-script-templates` crate, where they are built and tested with the engine; a new script
//! from a template is its source with the type renamed, plus the input actions it needs added
//! to the project's input map.

use anyhow::{Context as _, Result};

/// A starter script.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptTemplate {
    /// The type name in the source (renamed in the new script).
    pub type_name: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    /// What it does, in a sentence or two.
    pub description: &'static str,
    /// What to attach it to.
    pub attach_to: &'static str,
    pub source: &'static str,
    /// Input actions it reads, with default bindings, in groups ([`ScriptTemplate::actions`]).
    pub action_groups: &'static [&'static [Action]],
}

/// An input action and its default bindings.
pub type Action = (&'static str, &'static [&'static str]);

const MOVE: &[Action] = &[
    (
        "move_forward",
        &["key:KeyW", "key:ArrowUp", "pad_axis:LeftStickY+"],
    ),
    (
        "move_back",
        &["key:KeyS", "key:ArrowDown", "pad_axis:LeftStickY-"],
    ),
    (
        "move_left",
        &["key:KeyA", "key:ArrowLeft", "pad_axis:LeftStickX-"],
    ),
    (
        "move_right",
        &["key:KeyD", "key:ArrowRight", "pad_axis:LeftStickX+"],
    ),
];
const LOOK: &[Action] = &[
    ("look_left", &["pad_axis:RightStickX-"]),
    ("look_right", &["pad_axis:RightStickX+"]),
    ("look_up", &["pad_axis:RightStickY+"]),
    ("look_down", &["pad_axis:RightStickY-"]),
];
const JUMP: &[Action] = &[("jump", &["key:Space", "pad:South"])];
const SPRINT: &[Action] = &[("sprint", &["key:ShiftLeft", "pad:LeftStick"])];
const CROUCH: &[Action] = &[("crouch", &["key:ControlLeft", "key:KeyC", "pad:RightStick"])];
const DASH: &[Action] = &[("dash", &["key:KeyQ", "pad:East"])];
const ATTACK: &[Action] = &[("attack", &["mouse:Left", "pad:West"])];
const FIRE: &[Action] = &[("fire", &["mouse:Left", "pad:RightTrigger"])];
const AIM: &[Action] = &[("aim", &["mouse:Right", "pad:LeftTrigger"])];
const RELOAD: &[Action] = &[("reload", &["key:KeyR", "pad:North"])];
const LOCK_ON: &[Action] = &[("lock_on", &["key:Tab", "pad:RightBumper"])];

/// Every template, in the order the editor lists them.
pub const SCRIPT_TEMPLATES: &[ScriptTemplate] = &[
    ScriptTemplate {
        type_name: "FirstPersonController",
        title: "First person controller",
        category: "Player",
        description: "Mouse and gamepad look, walk, sprint, crouch, jump, climbing and a camera that moves like a head. Builds its own camera.",
        attach_to: "a CharacterBody3D with a capsule",
        source: include_str!("../../ab-script-templates/src/first_person.rs"),
        action_groups: &[MOVE, LOOK, JUMP, SPRINT, CROUCH],
    },
    ScriptTemplate {
        type_name: "ThirdPersonController",
        title: "Third person controller",
        category: "Player",
        description: "Orbiting camera on a spring arm, camera-relative movement, jump, sprint, dash, climbing, and the model's animations blended from the movement.",
        attach_to: "a CharacterBody3D whose child is a rigged model",
        source: include_str!("../../ab-script-templates/src/third_person.rs"),
        action_groups: &[MOVE, LOOK, JUMP, SPRINT, DASH],
    },
    ScriptTemplate {
        type_name: "Movement",
        title: "Movement",
        category: "Player",
        description: "Walk and run relative to the camera, with gravity: the simplest controller. Add Jump and Dash next to it.",
        attach_to: "a CharacterBody3D",
        source: include_str!("../../ab-script-templates/src/movement.rs"),
        action_groups: &[MOVE, SPRINT],
    },
    ScriptTemplate {
        type_name: "Jump",
        title: "Jump",
        category: "Player",
        description: "Coyote time, jump buffering, lower jumps when released early, optional double jump.",
        attach_to: "a CharacterBody3D moved by Movement (or your script)",
        source: include_str!("../../ab-script-templates/src/jump.rs"),
        action_groups: &[JUMP],
    },
    ScriptTemplate {
        type_name: "Dash",
        title: "Dash",
        category: "Player",
        description: "A quick burst with a cooldown, invulnerability while dashing and a dust trail.",
        attach_to: "a CharacterBody3D moved by Movement (or your script)",
        source: include_str!("../../ab-script-templates/src/dash.rs"),
        action_groups: &[MOVE, DASH],
    },
    ScriptTemplate {
        type_name: "AnimatedCharacter",
        title: "Animated character",
        category: "Animation",
        description: "Plays a rigged model's idle, walk, run, jump, fall and climb from how its body moves, blended and crossfaded; one-shot clips on signals.",
        attach_to: "a moving body whose child is a rigged model",
        source: include_str!("../../ab-script-templates/src/animated_character.rs"),
        action_groups: &[],
    },
    ScriptTemplate {
        type_name: "MeleeCombat",
        title: "Melee combat (combos)",
        category: "Combat",
        description: "Attacks chained into a combo with timing windows; hits damage what is in front, push it and shake the camera.",
        attach_to: "the player's body",
        source: include_str!("../../ab-script-templates/src/melee_combat.rs"),
        action_groups: &[ATTACK],
    },
    ScriptTemplate {
        type_name: "Health",
        title: "Health",
        category: "Combat",
        description: "Hit points damaged by a `damage` signal, invulnerability after hits, regeneration, and death with blood and a ragdoll.",
        attach_to: "anything that can be hurt",
        source: include_str!("../../ab-script-templates/src/health.rs"),
        action_groups: &[],
    },
    ScriptTemplate {
        type_name: "Gun",
        title: "Gun (with reloading)",
        category: "Combat",
        description: "Hitscan shooting, magazine and reloading, recoil, spread, muzzle flash, impacts and an ammo counter.",
        attach_to: "the player (with a camera)",
        source: include_str!("../../ab-script-templates/src/gun.rs"),
        action_groups: &[FIRE, AIM, RELOAD],
    },
    ScriptTemplate {
        type_name: "Crosshair",
        title: "Crosshair",
        category: "Combat",
        description: "A crosshair that spreads when moving and firing, turns red over targets and flashes on hits.",
        attach_to: "the player (next to Gun)",
        source: include_str!("../../ab-script-templates/src/crosshair.rs"),
        action_groups: &[],
    },
    ScriptTemplate {
        type_name: "AutoAim",
        title: "Auto aim / lock-on",
        category: "Combat",
        description: "Pulls the view onto the enemy nearest the centre while aiming, or locks on to one.",
        attach_to: "the player (next to a controller template)",
        source: include_str!("../../ab-script-templates/src/auto_aim.rs"),
        action_groups: &[AIM, LOCK_ON],
    },
    ScriptTemplate {
        type_name: "EnemyAi",
        title: "Enemy",
        category: "Characters",
        description: "Wanders, sees and hears the player, chases it along the navigation, attacks in reach, searches and gives up.",
        attach_to: "a CharacterBody3D with Health (and a rigged model)",
        source: include_str!("../../ab-script-templates/src/enemy.rs"),
        action_groups: &[],
    },
    ScriptTemplate {
        type_name: "Car",
        title: "Car",
        category: "Vehicles",
        description: "Raycast suspension, tyre grip, handbrake drifts, engine and brakes, a chase camera and getting back on its wheels.",
        attach_to: "a RigidBody3D with a box shape",
        source: include_str!("../../ab-script-templates/src/car.rs"),
        action_groups: &[MOVE, JUMP],
    },
];

impl ScriptTemplate {
    /// The input actions it reads, with their default bindings.
    pub fn actions(&self) -> impl Iterator<Item = &'static Action> {
        self.action_groups.iter().flat_map(|group| group.iter())
    }

    /// The source with the type (and its whole-word mentions) renamed to `name`.
    pub fn source_for(&self, name: &str) -> String {
        rename_word(self.source, self.type_name, name)
    }
}

/// Replaces whole-word occurrences of `from` (not inside longer identifiers).
fn rename_word(text: &str, from: &str, to: &str) -> String {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + from.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(is_ident) || after.is_some_and(is_ident) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        rest = &rest[at + from.len()..];
    }
    out.push_str(rest);
    out
}

/// Adds the actions the template reads that the input map (TOML text) does not have yet.
/// Returns the new text and the actions added.
pub fn add_actions(input_toml: &str, template: &ScriptTemplate) -> Result<(String, Vec<String>)> {
    let mut document: toml::Table = input_toml
        .parse()
        .context("the input map is not valid TOML")?;
    let actions = document
        .entry("actions")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("`actions` in the input map is not a table")?;
    let mut added = Vec::new();
    for (name, bindings) in template.actions() {
        if actions.contains_key(*name) {
            continue;
        }
        let mut action = toml::Table::new();
        action.insert(
            "bindings".into(),
            toml::Value::Array(
                bindings
                    .iter()
                    .map(|b| toml::Value::String((*b).into()))
                    .collect(),
            ),
        );
        actions.insert((*name).into(), toml::Value::Table(action));
        added.push((*name).to_owned());
    }
    if added.is_empty() {
        return Ok((input_toml.to_owned(), added));
    }
    // Keep the file's comments: append the new actions as their own tables.
    let mut text = input_toml.trim_end().to_owned();
    for name in &added {
        let bindings = template
            .actions()
            .find(|(n, _)| n == name)
            .map(|(_, b)| *b)
            .unwrap_or(&[]);
        let list: Vec<String> = bindings.iter().map(|b| format!("\"{b}\"")).collect();
        text.push_str(&format!(
            "\n\n[actions.{name}]\nbindings = [{}]",
            list.join(", ")
        ));
    }
    text.push('\n');
    Ok((text, added))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_defines_its_type_and_renames_cleanly() {
        for template in SCRIPT_TEMPLATES {
            let marker = format!("impl Script for {} {{", template.type_name);
            assert!(template.source.contains(&marker), "{}", template.title);
            let renamed = template.source_for("MyThing");
            assert!(
                renamed.contains("impl Script for MyThing {"),
                "{}",
                template.title
            );
            assert!(!renamed.contains(&marker));
            assert!(!template.description.is_empty() && !template.attach_to.is_empty());
        }
        let names: std::collections::HashSet<_> =
            SCRIPT_TEMPLATES.iter().map(|t| t.type_name).collect();
        assert_eq!(names.len(), SCRIPT_TEMPLATES.len(), "unique names");
    }

    #[test]
    fn renaming_leaves_longer_words_alone() {
        assert_eq!(
            rename_word("Gun GunHud gun Gun::new MyGun", "Gun", "Rifle"),
            "Rifle GunHud gun Rifle::new MyGun"
        );
    }

    #[test]
    fn actions_are_added_once_with_their_bindings() {
        let gun = SCRIPT_TEMPLATES
            .iter()
            .find(|t| t.type_name == "Gun")
            .unwrap();
        let start = "# my bindings\n[actions.fire]\nbindings = [\"key:KeyF\"]\n";
        let (text, added) = add_actions(start, gun).unwrap();
        assert_eq!(added, ["aim", "reload"], "fire is already there");
        assert!(text.starts_with("# my bindings"), "comments kept");
        let map: toml::Table = text.parse().unwrap();
        let reload = &map["actions"]["reload"]["bindings"];
        assert_eq!(reload.as_array().unwrap()[0].as_str(), Some("key:KeyR"));
        assert_eq!(
            map["actions"]["fire"]["bindings"][0].as_str(),
            Some("key:KeyF")
        );
        let (again, none) = add_actions(&text, gun).unwrap();
        assert!(none.is_empty() && again == text);
        let car = SCRIPT_TEMPLATES
            .iter()
            .find(|t| t.type_name == "Car")
            .unwrap();
        assert_eq!(add_actions("", car).unwrap().1.len(), 5);
    }
}
