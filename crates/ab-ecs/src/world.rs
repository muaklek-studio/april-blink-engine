use crate::bundle::Bundle;
use crate::command::Commands;
use crate::component::Component;
use crate::entity::{Entity, EntityAllocator};
use crate::query::{Query, QueryData, QueryFilter};
use crate::sparse_set::{ErasedStorage, SparseSet};
use std::any::{type_name, Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt;

/// Marker for singleton values stored in the world (time, input state, physics world, ...).
pub trait Resource: 'static {}

impl<T: 'static> Resource for T {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EcsError {
    #[error("entity {0} does not exist (despawned or never spawned)")]
    NoSuchEntity(Entity),
}

struct StorageEntry {
    name: &'static str,
    cell: RefCell<Box<dyn ErasedStorage>>,
}

struct ResourceEntry {
    name: &'static str,
    cell: RefCell<Box<dyn Any>>,
}

/// Owns entities, their components and global resources.
///
/// Read/write access through `&self` ([`World::get`], [`World::get_mut`], [`World::query`],
/// [`World::resource_mut`]) is checked at runtime per component/resource type: holding two
/// mutable borrows of the same type panics. Structural changes (spawn, insert, remove,
/// despawn) need `&mut self`; while iterating, defer them through [`World::commands`].
pub struct World {
    entities: EntityAllocator,
    storages: HashMap<TypeId, StorageEntry>,
    resources: HashMap<TypeId, ResourceEntry>,
    commands: RefCell<Commands>,
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: EntityAllocator::new(),
            storages: HashMap::new(),
            resources: HashMap::new(),
            commands: RefCell::new(Commands::new()),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Entities
    // ---------------------------------------------------------------------------------------

    pub fn spawn_empty(&mut self) -> Entity {
        self.entities.allocate()
    }

    /// Spawns an entity with every component in `bundle` (a tuple, e.g. `(A, B)` or `(A,)`).
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let entity = self.entities.allocate();
        bundle.insert_into(self, entity);
        entity
    }

    /// Removes the entity and all of its components. Returns `false` if it was not alive.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.deallocate(entity) {
            return false;
        }
        for entry in self.storages.values_mut() {
            entry.cell.get_mut().remove_entity(entity);
        }
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// Number of live entities.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter()
    }

    /// Despawns every entity. Resources are kept.
    pub fn clear_entities(&mut self) {
        // Free through the allocator so generations advance: a fresh allocator would hand
        // out the old ids again and revive stale handles.
        let live: Vec<Entity> = self.entities.iter().collect();
        for entity in live {
            self.entities.deallocate(entity);
        }
        self.storages.clear();
    }

    // ---------------------------------------------------------------------------------------
    // Components
    // ---------------------------------------------------------------------------------------

    /// Adds (or replaces) a component, returning the previous value.
    pub fn insert<T: Component>(
        &mut self,
        entity: Entity,
        component: T,
    ) -> Result<Option<T>, EcsError> {
        if !self.is_alive(entity) {
            return Err(EcsError::NoSuchEntity(entity));
        }
        Ok(self.storage_entry_mut::<T>().insert(entity, component))
    }

    /// Adds every component of `bundle` to an existing entity.
    pub fn insert_bundle<B: Bundle>(&mut self, entity: Entity, bundle: B) -> Result<(), EcsError> {
        if !self.is_alive(entity) {
            return Err(EcsError::NoSuchEntity(entity));
        }
        bundle.insert_into(self, entity);
        Ok(())
    }

    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        self.storages
            .get_mut(&TypeId::of::<T>())?
            .cell
            .get_mut()
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()
            .expect("storage type mismatch")
            .remove(entity)
    }

    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        self.storages
            .get(&TypeId::of::<T>())
            .is_some_and(|entry| borrow_storage(entry).contains_entity(entity))
    }

    /// Shared borrow of a component. Panics if the component type is mutably borrowed.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.storage::<T>()?, |set| set.get(entity)).ok()
    }

    /// Exclusive borrow of a component. Panics if the component type is already borrowed.
    pub fn get_mut<T: Component>(&self, entity: Entity) -> Option<RefMut<'_, T>> {
        RefMut::filter_map(self.storage_mut::<T>()?, |set| set.get_mut(entity)).ok()
    }

    /// Copies a component out (convenient for small `Copy` components such as transforms).
    pub fn get_copied<T: Component + Copy>(&self, entity: Entity) -> Option<T> {
        self.get::<T>(entity).map(|c| *c)
    }

    /// Shared borrow of the whole storage for `T`, or `None` if `T` was never inserted.
    pub fn storage<T: Component>(&self) -> Option<Ref<'_, SparseSet<T>>> {
        let entry = self.storages.get(&TypeId::of::<T>())?;
        Some(Ref::map(borrow_storage(entry), |s| {
            s.as_any()
                .downcast_ref::<SparseSet<T>>()
                .expect("storage type mismatch")
        }))
    }

    /// Exclusive borrow of the whole storage for `T`, or `None` if `T` was never inserted.
    pub fn storage_mut<T: Component>(&self) -> Option<RefMut<'_, SparseSet<T>>> {
        let entry = self.storages.get(&TypeId::of::<T>())?;
        Some(RefMut::map(borrow_storage_mut(entry), |s| {
            s.as_any_mut()
                .downcast_mut::<SparseSet<T>>()
                .expect("storage type mismatch")
        }))
    }

    fn storage_entry_mut<T: Component>(&mut self) -> &mut SparseSet<T> {
        self.storages
            .entry(TypeId::of::<T>())
            .or_insert_with(|| StorageEntry {
                name: type_name::<T>(),
                cell: RefCell::new(Box::new(SparseSet::<T>::new())),
            })
            .cell
            .get_mut()
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()
            .expect("storage type mismatch")
    }

    /// Iterates entities matching `Q`. See the crate docs for supported query shapes.
    pub fn query<Q: QueryData>(&self) -> Query<'_, Q, ()> {
        Query::new(self)
    }

    /// Like [`World::query`] with an extra filter, e.g. `Without<Parent>`.
    pub fn query_filtered<Q: QueryData, F: QueryFilter>(&self) -> Query<'_, Q, F> {
        Query::new(self)
    }

    // ---------------------------------------------------------------------------------------
    // Resources
    // ---------------------------------------------------------------------------------------

    /// Inserts (or replaces) a resource, returning the previous value.
    pub fn insert_resource<R: Resource>(&mut self, resource: R) -> Option<R> {
        let old = self.resources.insert(
            TypeId::of::<R>(),
            ResourceEntry {
                name: type_name::<R>(),
                cell: RefCell::new(Box::new(resource)),
            },
        )?;
        old.cell.into_inner().downcast::<R>().ok().map(|b| *b)
    }

    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        let entry = self.resources.remove(&TypeId::of::<R>())?;
        entry.cell.into_inner().downcast::<R>().ok().map(|b| *b)
    }

    pub fn contains_resource<R: Resource>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    /// Shared borrow of a resource. Panics if it is currently mutably borrowed.
    pub fn resource<R: Resource>(&self) -> Option<Ref<'_, R>> {
        let entry = self.resources.get(&TypeId::of::<R>())?;
        let borrowed = entry
            .cell
            .try_borrow()
            .unwrap_or_else(|_| panic!("resource `{}` is already mutably borrowed", entry.name));
        Some(Ref::map(borrowed, |r| {
            r.downcast_ref::<R>().expect("resource type mismatch")
        }))
    }

    /// Exclusive borrow of a resource. Panics if it is currently borrowed.
    pub fn resource_mut<R: Resource>(&self) -> Option<RefMut<'_, R>> {
        let entry = self.resources.get(&TypeId::of::<R>())?;
        let borrowed = entry
            .cell
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("resource `{}` is already borrowed", entry.name));
        Some(RefMut::map(borrowed, |r| {
            r.downcast_mut::<R>().expect("resource type mismatch")
        }))
    }

    /// Returns the resource, inserting `make()` first if it is missing.
    pub fn resource_or_insert_with<R: Resource>(&mut self, make: impl FnOnce() -> R) -> &mut R {
        self.resources
            .entry(TypeId::of::<R>())
            .or_insert_with(|| ResourceEntry {
                name: type_name::<R>(),
                cell: RefCell::new(Box::new(make())),
            })
            .cell
            .get_mut()
            .downcast_mut::<R>()
            .expect("resource type mismatch")
    }

    // ---------------------------------------------------------------------------------------
    // Deferred commands
    // ---------------------------------------------------------------------------------------

    /// The world's deferred command queue. Usable while the world is borrowed (e.g. inside a
    /// query loop); applied by [`World::flush`].
    pub fn commands(&self) -> RefMut<'_, Commands> {
        self.commands
            .try_borrow_mut()
            .expect("world command queue is already borrowed")
    }

    /// Applies all queued commands, including commands queued while applying.
    pub fn flush(&mut self) {
        loop {
            let mut commands = std::mem::take(self.commands.get_mut());
            if commands.is_empty() {
                break;
            }
            commands.apply(self);
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut components: Vec<_> = self.storages.values().map(|e| e.name).collect();
        components.sort_unstable();
        let mut resources: Vec<_> = self.resources.values().map(|e| e.name).collect();
        resources.sort_unstable();
        f.debug_struct("World")
            .field("entities", &self.entities.len())
            .field("components", &components)
            .field("resources", &resources)
            .finish()
    }
}

fn borrow_storage(entry: &StorageEntry) -> Ref<'_, Box<dyn ErasedStorage>> {
    entry
        .cell
        .try_borrow()
        .unwrap_or_else(|_| panic!("component `{}` is already mutably borrowed", entry.name))
}

fn borrow_storage_mut(entry: &StorageEntry) -> RefMut<'_, Box<dyn ErasedStorage>> {
    entry
        .cell
        .try_borrow_mut()
        .unwrap_or_else(|_| panic!("component `{}` is already borrowed", entry.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Pos(f32);
    #[derive(Debug, PartialEq)]
    struct Name(&'static str);
    #[derive(Debug, PartialEq, Default)]
    struct Score(u32);

    #[test]
    fn spawn_get_insert_remove() {
        let mut world = World::new();
        let e = world.spawn((Pos(1.0), Name("a")));
        assert_eq!(*world.get::<Pos>(e).unwrap(), Pos(1.0));
        assert!(world.has::<Name>(e));
        assert!(!world.has::<Score>(e));

        assert_eq!(world.insert(e, Pos(2.0)), Ok(Some(Pos(1.0))));
        assert_eq!(world.remove::<Name>(e), Some(Name("a")));
        assert!(!world.has::<Name>(e));
        assert_eq!(world.get_copied::<Pos>(e), Some(Pos(2.0)));
    }

    #[test]
    fn despawn_removes_all_components() {
        let mut world = World::new();
        let e = world.spawn((Pos(1.0), Name("a")));
        let other = world.spawn((Pos(5.0),));
        assert!(world.despawn(e));
        assert!(!world.is_alive(e));
        assert!(world.get::<Pos>(e).is_none());
        assert_eq!(world.storage::<Name>().unwrap().len(), 0);
        assert_eq!(world.get_copied::<Pos>(other), Some(Pos(5.0)));
        assert!(!world.despawn(e));
    }

    #[test]
    fn stale_entity_cannot_receive_components() {
        let mut world = World::new();
        let e = world.spawn_empty();
        world.despawn(e);
        let reused = world.spawn_empty();
        assert_eq!(reused.index(), e.index());
        assert_eq!(world.insert(e, Pos(0.0)), Err(EcsError::NoSuchEntity(e)));
        assert!(world.get::<Pos>(reused).is_none());
    }

    #[test]
    fn get_mut_writes_through() {
        let mut world = World::new();
        let e = world.spawn((Score(1),));
        world.get_mut::<Score>(e).unwrap().0 += 41;
        assert_eq!(world.get::<Score>(e).unwrap().0, 42);
    }

    #[test]
    fn different_component_types_can_be_borrowed_together() {
        let mut world = World::new();
        let e = world.spawn((Pos(1.0), Score(0)));
        let pos = world.get::<Pos>(e).unwrap();
        let mut score = world.get_mut::<Score>(e).unwrap();
        score.0 = pos.0 as u32;
    }

    #[test]
    #[should_panic(expected = "already mutably borrowed")]
    fn conflicting_component_borrow_panics() {
        let mut world = World::new();
        let e = world.spawn((Pos(1.0),));
        let _write = world.get_mut::<Pos>(e).unwrap();
        let _read = world.get::<Pos>(e);
    }

    #[test]
    fn resources_round_trip() {
        let mut world = World::new();
        assert!(world.resource::<Score>().is_none());
        assert!(world.insert_resource(Score(1)).is_none());
        world.resource_mut::<Score>().unwrap().0 += 1;
        assert_eq!(world.resource::<Score>().unwrap().0, 2);
        assert_eq!(world.insert_resource(Score(10)), Some(Score(2)));
        world.resource_or_insert_with(Score::default).0 += 1;
        assert_eq!(world.remove_resource::<Score>(), Some(Score(11)));
        assert!(!world.contains_resource::<Score>());
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn conflicting_resource_borrow_panics() {
        let mut world = World::new();
        world.insert_resource(Score(0));
        let _a = world.resource::<Score>();
        let _b = world.resource_mut::<Score>();
    }

    #[test]
    fn commands_are_deferred_until_flush() {
        let mut world = World::new();
        let a = world.spawn((Pos(1.0),));
        let b = world.spawn((Pos(-1.0),));
        for (e, pos) in world.query::<(Entity, &Pos)>().iter() {
            if pos.0 < 0.0 {
                world.commands().despawn(e);
            }
        }
        assert!(world.is_alive(b));
        world.flush();
        assert!(world.is_alive(a));
        assert!(!world.is_alive(b));
    }

    #[test]
    fn clear_entities_invalidates_old_handles() {
        let mut world = World::new();
        let old = world.spawn((Pos(1.0),));
        world.clear_entities();
        assert!(!world.is_alive(old));
        assert_eq!(world.entity_count(), 0);
        let new = world.spawn((Pos(2.0),));
        assert_eq!(new.index(), old.index());
        assert!(
            !world.is_alive(old),
            "stale handle must not alias the new entity"
        );
        assert!(world.get::<Pos>(old).is_none());
        assert_eq!(world.get_copied::<Pos>(new), Some(Pos(2.0)));
    }

    #[test]
    fn flush_applies_commands_queued_by_commands() {
        let mut world = World::new();
        world.commands().add(|w: &mut World| {
            w.commands().add(|w: &mut World| {
                w.spawn((Score(7),));
            });
        });
        world.flush();
        assert_eq!(world.entity_count(), 1);
    }
}
