//! The methods script libraries call, by name.

mod nodes;
mod physics;
mod properties;
mod ui;
mod world;

use super::call::{Call, CallResult};
use ab_script_api::MethodId;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) use properties::names as property_names;

/// A method scripts can call.
pub(crate) struct Binding {
    pub name: &'static str,
    pub run: fn(&mut Call<'_>) -> CallResult,
}

/// Every binding, by name and by id (index + 1; 0 means unknown).
pub(crate) struct Table {
    list: Vec<Binding>,
    ids: HashMap<&'static str, u32>,
}

impl Table {
    pub fn id(&self, name: &str) -> MethodId {
        self.ids
            .get(name)
            .map_or(MethodId::UNKNOWN, |&id| MethodId(id))
    }

    pub fn get(&self, id: MethodId) -> Option<&Binding> {
        self.list.get((id.0 as usize).checked_sub(1)?)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.list.iter().map(|b| b.name)
    }
}

/// Collects bindings while the table is built.
pub(crate) struct Builder {
    list: Vec<Binding>,
}

impl Builder {
    pub fn add(&mut self, name: &'static str, run: fn(&mut Call<'_>) -> CallResult) {
        debug_assert!(
            !self.list.iter().any(|b| b.name == name),
            "binding `{name}` defined twice"
        );
        self.list.push(Binding { name, run });
    }
}

pub(crate) fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut builder = Builder { list: Vec::new() };
        nodes::add(&mut builder);
        properties::add(&mut builder);
        physics::add(&mut builder);
        ui::add(&mut builder);
        world::add(&mut builder);
        let ids = builder
            .list
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name, i as u32 + 1))
            .collect();
        Table {
            list: builder.list,
            ids,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_resolve_to_their_bindings() {
        let table = table();
        assert!(table.list.len() > 100, "{} bindings", table.list.len());
        for binding in &table.list {
            let id = table.id(binding.name);
            assert_eq!(table.get(id).map(|b| b.name), Some(binding.name));
        }
        assert_eq!(table.id("no.such_method"), MethodId::UNKNOWN);
        assert!(table.get(MethodId::UNKNOWN).is_none());
        assert!(table.get(MethodId(u32::MAX)).is_none());
    }

    /// Every method and property the SDK uses exists here (the SDK names them in string
    /// literals; this keeps both sides in step).
    #[test]
    fn the_sdk_only_uses_existing_bindings() {
        let sdk = concat!(env!("CARGO_MANIFEST_DIR"), "/../ab-script/src");
        let mut missing = Vec::new();
        let properties = property_names();
        for entry in std::fs::read_dir(sdk).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            // `method!("name")`, `get/set(self.node, "path")` and
            // `property!(getter, setter, Type, "path", "doc")`.
            for (marker, field, known) in [
                ("method!(\"", 0, None),
                ("self.node, \"", 0, Some(&properties)),
                ("property!(", 1, Some(&properties)),
            ] {
                for piece in text.split(marker).skip(1) {
                    let name = piece.split('"').nth(field).unwrap();
                    let exists = match known {
                        None => table().id(name) != MethodId::UNKNOWN,
                        Some(properties) => properties.contains(&name),
                    };
                    if !exists {
                        missing.push(format!("{}: {name}", path.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "the engine lacks:\n{}",
            missing.join("\n")
        );
    }
}
