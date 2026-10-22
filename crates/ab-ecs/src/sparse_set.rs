use crate::entity::Entity;
use std::any::Any;

const EMPTY: u32 = u32::MAX;

/// Component storage: a sparse array indexed by entity index pointing into packed arrays.
///
/// Insert, remove and lookup are O(1); iteration walks contiguous memory. Removal uses
/// `swap_remove`, so iteration order is not stable across removals.
#[derive(Debug)]
pub struct SparseSet<T> {
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    values: Vec<T>,
}

impl<T> SparseSet<T> {
    pub fn new() -> Self {
        Self {
            sparse: Vec::new(),
            entities: Vec::new(),
            values: Vec::new(),
        }
    }

    fn dense_index(&self, entity: Entity) -> Option<usize> {
        let slot = *self.sparse.get(entity.index() as usize)?;
        if slot == EMPTY {
            return None;
        }
        let slot = slot as usize;
        // The dense entity keeps the generation, so stale handles never match.
        (self.entities[slot] == entity).then_some(slot)
    }

    /// Inserts or replaces the value for `entity`, returning the previous value.
    pub fn insert(&mut self, entity: Entity, value: T) -> Option<T> {
        let index = entity.index() as usize;
        if index >= self.sparse.len() {
            self.sparse.resize(index + 1, EMPTY);
        }
        let slot = self.sparse[index];
        if slot != EMPTY {
            let slot = slot as usize;
            if self.entities[slot] == entity {
                return Some(std::mem::replace(&mut self.values[slot], value));
            }
            // A stale entry from a previous generation: overwrite it in place.
            self.entities[slot] = entity;
            self.values[slot] = value;
            return None;
        }
        self.sparse[index] = self.values.len() as u32;
        self.entities.push(entity);
        self.values.push(value);
        None
    }

    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let slot = self.dense_index(entity)?;
        self.sparse[entity.index() as usize] = EMPTY;
        let last = self.values.len() - 1;
        if slot != last {
            let moved = self.entities[last];
            self.sparse[moved.index() as usize] = slot as u32;
        }
        self.entities.swap_remove(slot);
        Some(self.values.swap_remove(slot))
    }

    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.dense_index(entity).map(|i| &self.values[i])
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        self.dense_index(entity).map(|i| &mut self.values[i])
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.dense_index(entity).is_some()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Entities in dense order (parallel to [`SparseSet::values`]).
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn values(&self) -> &[T] {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut [T] {
        &mut self.values
    }

    /// The packed entities and their values side by side (`entities()[i]` owns
    /// `values[i]`), e.g. to process the values in parallel while knowing their owners.
    pub fn parts_mut(&mut self) -> (&[Entity], &mut [T]) {
        (&self.entities, &mut self.values)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.entities.iter().copied().zip(self.values.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.entities.iter().copied().zip(self.values.iter_mut())
    }

    pub fn clear(&mut self) {
        self.sparse.clear();
        self.entities.clear();
        self.values.clear();
    }

    /// Raw pointer to the value of `entity`. Used by queries that hand out several disjoint
    /// `&mut T` from one storage borrow.
    pub(crate) fn get_ptr(&mut self, entity: Entity) -> Option<*mut T> {
        self.dense_index(entity)
            .map(|i| self.values.as_mut_ptr().wrapping_add(i))
    }
}

impl<T> Default for SparseSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-erased view of a `SparseSet<T>` so the world can manage storages generically.
pub(crate) trait ErasedStorage: Any {
    fn remove_entity(&mut self, entity: Entity) -> bool;
    fn contains_entity(&self, entity: Entity) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: 'static> ErasedStorage for SparseSet<T> {
    fn remove_entity(&mut self, entity: Entity) -> bool {
        self.remove(entity).is_some()
    }

    fn contains_entity(&self, entity: Entity) -> bool {
        self.contains(entity)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityAllocator;

    #[test]
    fn insert_get_replace() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        let mut set = SparseSet::new();
        assert_eq!(set.insert(e, 1), None);
        assert_eq!(set.insert(e, 2), Some(1));
        assert_eq!(set.get(e), Some(&2));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn remove_keeps_other_entries_reachable() {
        let mut alloc = EntityAllocator::new();
        let [a, b, c] = [alloc.allocate(), alloc.allocate(), alloc.allocate()];
        let mut set = SparseSet::new();
        set.insert(a, 'a');
        set.insert(b, 'b');
        set.insert(c, 'c');

        assert_eq!(set.remove(a), Some('a'));
        assert_eq!(set.get(a), None);
        assert_eq!(set.get(b), Some(&'b'));
        assert_eq!(set.get(c), Some(&'c'));
        assert_eq!(set.len(), 2);
        assert_eq!(set.remove(a), None);
    }

    #[test]
    fn stale_generation_does_not_match() {
        let mut alloc = EntityAllocator::new();
        let old = alloc.allocate();
        let mut set = SparseSet::new();
        set.insert(old, 1);
        alloc.deallocate(old);
        let new = alloc.allocate();
        assert_eq!(new.index(), old.index());

        assert_eq!(set.get(new), None);
        assert_eq!(set.insert(new, 2), None);
        assert_eq!(set.get(old), None);
        assert_eq!(set.get(new), Some(&2));
        assert_eq!(set.len(), 1);
    }
}
