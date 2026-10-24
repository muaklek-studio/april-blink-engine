//! Prefabs: scene files used as reusable objects, with changes kept per instance.
//!
//! Any scene file can be instanced (`instance = "prefabs/lamp.abscene"`). An instance may
//! change its nodes without touching the prefab; the changes are saved with the instance as
//! overrides, one table per changed node, naming it by path from the prefab's root and
//! holding only the keys that differ:
//!
//! ```toml
//! [[node]]
//! name = "Lamp2"
//! parent = "."
//! instance = "prefabs/lamp.abscene"
//! position = [4.0, 0.0, 0.0]
//! overrides = [{ path = "Light", light = { color = "#ff8040" } }, { path = "Shade", visible = false }]
//! ```
//!
//! A key the instance went back to the default for, while the prefab sets it, is written
//! as `{}` (unset): `{ path = "Shade", scale = {}, body = { mass = {} } }`.
//!
//! Loading builds the prefab as it is now with the overrides on top, so a change to the
//! prefab reaches every instance while each keeps its own changes. Saving compares each
//! instance with its prefab again and writes what differs.
//!
//! What can go wrong is kept, not lost: an override whose node the prefab no longer has
//! stays with the instance (and is reported), and an instance whose file is missing or
//! broken loads as an empty placeholder that saves back exactly as it was written.

use super::format::{NodeDesc, SceneFile};
use super::SceneInstance;
use ab_asset::AssetServer;
use ab_scene::{NodeHandle, SceneTree};

/// Keys an override cannot change: they make the prefab what it is.
const FIXED_KEYS: &[&str] = &["name", "parent", "type", "instance", "overrides", "path"];
/// Keys of the instance itself (written on the instance node, not as overrides of the root).
const INSTANCE_KEYS: &[&str] = &["position", "rotation", "scale"];

/// Overrides of an instance that match no node of its prefab (renamed or removed there):
/// kept to be saved again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnmatchedOverrides(pub Vec<toml::Table>);

/// An instance whose scene could not be loaded: an empty node standing in for it.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingInstance {
    pub error: String,
    /// The overrides as written, saved back unchanged.
    pub overrides: Vec<toml::Table>,
}

/// Each node's path from the scene's root (`.` for the root itself), in file order.
pub fn node_paths(scene: &SceneFile) -> Vec<String> {
    scene
        .nodes
        .iter()
        .map(|node| match node.parent.as_deref() {
            None => ".".to_owned(),
            Some(".") => node.name.clone(),
            Some(parent) => format!("{parent}/{}", node.name),
        })
        .collect()
}

fn to_table(desc: &NodeDesc) -> toml::Table {
    match toml::Value::try_from(desc) {
        Ok(toml::Value::Table(table)) => table,
        _ => toml::Table::new(),
    }
}

/// `over` merged into `base`: tables key by key, anything else replaced. An empty table
/// (`{}`) unsets the key (the instance went back to the default the prefab overrode; a
/// key the prefab no longer sets stays unset).
fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (_, toml::Value::Table(over)) if over.is_empty() => {
                base.remove(key);
            }
            (Some(toml::Value::Table(base)), toml::Value::Table(over)) => merge(base, over),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// The override value that unsets a key (see [`merge`]).
fn unset() -> toml::Value {
    toml::Value::Table(toml::Table::new())
}

/// Applies `overrides` to the prefab `scene`. Returns the ones it could not apply, with
/// why (no such node, a fixed key, a value of the wrong kind).
pub fn apply_overrides(
    scene: &mut SceneFile,
    overrides: &[toml::Table],
) -> Vec<(toml::Table, String)> {
    let paths = node_paths(scene);
    let mut unmatched = Vec::new();
    for o in overrides {
        let Some(path) = o.get("path").and_then(|p| p.as_str()) else {
            unmatched.push((o.clone(), "an override needs `path`".to_owned()));
            continue;
        };
        let Some(index) = paths.iter().position(|p| p == path) else {
            unmatched.push((o.clone(), format!("the prefab has no node `{path}`")));
            continue;
        };
        if let Some(key) = o
            .keys()
            .find(|k| *k != "path" && FIXED_KEYS.contains(&k.as_str()))
        {
            unmatched.push((o.clone(), format!("`{key}` cannot be overridden")));
            continue;
        }
        let mut table = to_table(&scene.nodes[index]);
        let mut fields = o.clone();
        fields.remove("path");
        merge(&mut table, &fields);
        match toml::Value::Table(table).try_into::<NodeDesc>() {
            Ok(mut desc) => {
                // `script` and `scripts` as saving writes them.
                desc.set_script_list(desc.script_list());
                scene.nodes[index] = desc;
            }
            Err(err) => unmatched.push((o.clone(), format!("`{path}`: {err}"))),
        }
    }
    unmatched
}

/// The overrides that turn `prefab` into `live` (the same nodes, changed): per node, the
/// keys whose values differ. A key the prefab sets and the instance no longer has (back to
/// its default) is written as `{}`, which unsets it on load.
pub fn diff_overrides(prefab: &SceneFile, live: &SceneFile) -> Vec<toml::Table> {
    let prefab_paths = node_paths(prefab);
    let live_paths = node_paths(live);
    let mut overrides = Vec::new();
    for (index, path) in prefab_paths.iter().enumerate() {
        let Some(live_index) = live_paths.iter().position(|p| p == path) else {
            continue;
        };
        let (before, after) = (
            to_table(&prefab.nodes[index]),
            to_table(&live.nodes[live_index]),
        );
        let mut changed = toml::Table::new();
        let skip =
            |key: &str| FIXED_KEYS.contains(&key) || (path == "." && INSTANCE_KEYS.contains(&key));
        for (key, value) in &after {
            if !skip(key) && before.get(key) != Some(value) {
                changed.insert(key.clone(), diff_value(before.get(key), value));
            }
        }
        for key in before.keys() {
            if !skip(key) && !after.contains_key(key) {
                changed.insert(key.clone(), unset());
            }
        }
        if !changed.is_empty() {
            let mut table = toml::Table::new();
            table.insert("path".into(), path.clone().into());
            table.extend(changed);
            overrides.push(table);
        }
    }
    overrides
}

/// For tables, only the keys that changed (merged back on load), and the dropped ones unset.
fn diff_value(before: Option<&toml::Value>, after: &toml::Value) -> toml::Value {
    match (before, after) {
        (Some(toml::Value::Table(b)), toml::Value::Table(a)) => {
            let mut changed = toml::Table::new();
            for (key, value) in a {
                if b.get(key) != Some(value) {
                    changed.insert(key.clone(), diff_value(b.get(key), value));
                }
            }
            for key in b.keys() {
                if !a.contains_key(key) {
                    changed.insert(key.clone(), unset());
                }
            }
            toml::Value::Table(changed)
        }
        _ => after.clone(),
    }
}

/// The prefab scene file at `path`, as on disk now.
pub fn read_prefab(tree: &SceneTree, path: &str) -> Result<SceneFile, String> {
    let assets = tree
        .world()
        .resource::<AssetServer>()
        .ok_or("the AssetServer resource is missing")?;
    let bytes = assets.read(path).map_err(|e| e.to_string())?;
    SceneFile::from_toml(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("{path}: {e}"))
}

/// What to save as `node`'s overrides (an instance of `path`).
pub(super) fn instance_overrides(
    tree: &SceneTree,
    node: NodeHandle,
    path: &str,
) -> Vec<toml::Table> {
    if let Some(missing) = tree.component::<MissingInstance>(node) {
        return missing.overrides.clone();
    }
    if !super::is_scene_path(path) {
        return Vec::new();
    }
    let unmatched = tree
        .component::<UnmatchedOverrides>(node)
        .map(|u| u.0.clone())
        .unwrap_or_default();
    let Ok(prefab) = read_prefab(tree, path) else {
        return unmatched;
    };
    let live = super::save_contents(tree, node);
    let mut overrides = diff_overrides(&prefab, &live);
    overrides.extend(unmatched);
    overrides
}

/// Whether `node` is the root of an instance of a scene file (not a model).
pub fn is_prefab_instance(tree: &SceneTree, node: NodeHandle) -> bool {
    tree.component::<SceneInstance>(node)
        .is_some_and(|i| super::is_scene_path(&i.0))
}

#[cfg(test)]
mod tests {
    use super::super::format::BodyDesc;
    use super::*;

    const LAMP: &str = r##"
[[node]]
name = "Lamp"
type = "Node3D"

[[node]]
name = "Light"
parent = "."
type = "OmniLight3D"
light = { color = "#ffffff", energy = 2.0 }

[[node]]
name = "Shade"
parent = "."
type = "MeshInstance3D"
visible = false
mesh = { primitive = "cube" }
material = { color = "#808080", roughness = 0.5 }
"##;

    #[test]
    fn overrides_apply_and_come_back_from_a_diff() {
        let prefab = SceneFile::from_toml(LAMP).unwrap();
        assert_eq!(node_paths(&prefab), [".", "Light", "Shade"]);
        let overrides: Vec<toml::Table> = vec![
            toml::from_str("path = \"Light\"\nlight = { color = \"#ff8040\" }").unwrap(),
            toml::from_str("path = \"Shade\"\nvisible = true").unwrap(),
        ];
        let mut live = prefab.clone();
        assert!(apply_overrides(&mut live, &overrides).is_empty());
        let light = live.nodes[1].light.as_ref().unwrap();
        assert_eq!(light.energy, Some(2.0), "the prefab's other keys stay");
        assert_eq!(
            format!("{:?}", light.color),
            format!("{:?}", Some(super::super::ColorDesc::Hex("#ff8040".into())))
        );
        assert_eq!(live.nodes[2].visible, Some(true));

        // The diff gives back just those changes.
        let diff = diff_overrides(&prefab, &live);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0]["path"].as_str(), Some("Light"));
        let light = diff[0]["light"].as_table().unwrap();
        assert_eq!(light.keys().collect::<Vec<_>>(), ["color"]);
        assert!(diff_overrides(&prefab, &prefab).is_empty());
    }

    /// Keys the instance no longer has (back to the default) come back from the diff: the
    /// prefab's value does not return on load.
    #[test]
    fn dropped_keys_round_trip() {
        const PREFAB: &str = r##"
[[node]]
name = "Crate"
type = "Node3D"

[[node]]
name = "Body"
parent = "."
type = "RigidBody3D"
position = [0.0, 2.0, 0.0]
scale = [2.0, 2.0, 2.0]
script = "Spin"
body = { mass = 5.0, friction = 0.2 }

[[node]]
name = "Mesh"
parent = "Body"
type = "MeshInstance3D"
cast_shadows = false
mesh = { primitive = "cube" }
"##;
        let prefab = SceneFile::from_toml(PREFAB).unwrap();
        let mut live = prefab.clone();
        let body = &mut live.nodes[1];
        body.position = None;
        body.scale = None;
        body.set_script_list(vec!["Spin".into(), "Wobble".into()]);
        body.body.as_mut().unwrap().mass = None;
        live.nodes[2].cast_shadows = None;

        let diff = diff_overrides(&prefab, &live);
        let mut loaded = prefab.clone();
        let unmatched = apply_overrides(&mut loaded, &diff);
        assert!(unmatched.is_empty(), "{unmatched:?}");
        assert_eq!(loaded, live, "overrides: {diff:?}");

        // A table the instance dropped entirely.
        let mut live = prefab.clone();
        live.nodes[1].body = None;
        live.nodes[1].set_script_list(Vec::new());
        let diff = diff_overrides(&prefab, &live);
        let mut loaded = prefab.clone();
        assert!(apply_overrides(&mut loaded, &diff).is_empty());
        assert_eq!(loaded.nodes[1].script_list(), Vec::<String>::new());
        assert_eq!(loaded.nodes[1].body.clone().unwrap_or_default(), BodyDesc::default());
    }

    #[test]
    fn bad_overrides_are_reported_not_applied() {
        let mut prefab = SceneFile::from_toml(LAMP).unwrap();
        let bad: Vec<toml::Table> = vec![
            toml::from_str("path = \"Gone\"\nvisible = false").unwrap(),
            toml::from_str("path = \"Light\"\nname = \"Other\"").unwrap(),
            toml::from_str("visible = false").unwrap(),
            toml::from_str("path = \"Light\"\nlight = 3").unwrap(),
        ];
        let unmatched = apply_overrides(&mut prefab, &bad);
        let reasons: Vec<_> = unmatched.iter().map(|(_, r)| r.as_str()).collect();
        assert!(reasons[0].contains("no node `Gone`"));
        assert!(reasons[1].contains("`name` cannot be overridden"));
        assert!(reasons[2].contains("needs `path`"));
        assert!(reasons[3].contains("`Light`"));
        assert_eq!(
            prefab,
            SceneFile::from_toml(LAMP).unwrap(),
            "nothing changed"
        );
    }
}
