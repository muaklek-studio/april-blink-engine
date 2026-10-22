use crate::script::{Guarded, NodeScript};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Creates a fresh instance of a registered script.
pub type ScriptFactory = Arc<dyn Fn() -> Box<dyn NodeScript> + Send + Sync>;

/// ECS resource: scripts by name. Attaching scripts by name
/// ([`SceneTree::set_script_named`](crate::SceneTree::set_script_named)) lets scene files
/// refer to them and lets hot reload swap in new versions.
#[derive(Default, Clone)]
pub struct ScriptRegistry {
    factories: BTreeMap<String, ScriptFactory>,
    fallback: Option<ScriptFactory>,
}

/// Monomorphised in the crate that registers `T`, so the panic guard uses that crate's `std`.
fn create<T: NodeScript + Default>() -> Box<dyn NodeScript> {
    Box::new(Guarded::new(T::default()))
}

impl ScriptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T` (built with `Default`) under `name`, replacing any previous entry.
    pub fn register<T: NodeScript + Default>(&mut self, name: &str) -> &mut Self {
        self.register_factory(name, create::<T>)
    }

    /// Registers a script made by `factory` (e.g. one from a script library).
    pub fn register_factory(
        &mut self,
        name: &str,
        factory: impl Fn() -> Box<dyn NodeScript> + Send + Sync + 'static,
    ) -> &mut Self {
        self.factories.insert(name.to_owned(), Arc::new(factory));
        self
    }

    /// Takes over every entry of `other` (newer versions win); keeps this registry's
    /// fallback unless `other` has one.
    pub fn extend(&mut self, other: ScriptRegistry) {
        self.factories.extend(other.factories);
        if other.fallback.is_some() {
            self.fallback = other.fallback;
        }
    }

    /// A new instance of the script registered as `name`, or of the fallback.
    pub fn create(&self, name: &str) -> Option<Box<dyn NodeScript>> {
        self.factories
            .get(name)
            .or(self.fallback.as_ref())
            .map(|factory| factory())
    }

    /// Creates a stand-in for every name without an entry (an editor showing scenes whose
    /// game scripts are not loaded). [`ScriptRegistry::contains`] still reports only real
    /// entries.
    pub fn set_fallback(&mut self, factory: Option<ScriptFactory>) -> &mut Self {
        self.fallback = factory;
        self
    }

    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.factories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl fmt::Debug for ScriptRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.factories.keys()).finish()
    }
}
