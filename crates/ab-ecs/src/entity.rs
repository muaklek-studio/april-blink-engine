use std::fmt;

/// Generational entity identifier.
///
/// `index` addresses a slot; `generation` is bumped every time the slot is freed, so a handle
/// to a despawned entity never matches the entity that later reuses its slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// Placeholder that never refers to a live entity.
    pub const PLACEHOLDER: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
    };

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the entity into a `u64` (generation in the high bits). Useful across FFI.
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Hands out entity ids and recycles freed slots (LIFO free list).
#[derive(Debug, Default)]
pub struct EntityAllocator {
    /// Current generation of every slot ever allocated.
    generations: Vec<u32>,
    /// Whether the slot currently holds a live entity.
    alive: Vec<bool>,
    free: Vec<u32>,
    len: usize,
}

impl EntityAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocate(&mut self) -> Entity {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let i = index as usize;
            self.alive[i] = true;
            return Entity {
                index,
                generation: self.generations[i],
            };
        }
        let index = u32::try_from(self.generations.len())
            .ok()
            .filter(|&i| i != u32::MAX)
            .expect("entity index space exhausted");
        self.generations.push(0);
        self.alive.push(true);
        Entity {
            index,
            generation: 0,
        }
    }

    /// Frees `entity`. Returns `false` if it was already dead or stale.
    pub fn deallocate(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let i = entity.index as usize;
        self.alive[i] = false;
        // A slot whose generation would wrap is retired instead of risking ABA aliasing.
        if let Some(next) = self.generations[i].checked_add(1) {
            self.generations[i] = next;
            self.free.push(entity.index);
        }
        self.len -= 1;
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        let i = entity.index as usize;
        i < self.alive.len() && self.alive[i] && self.generations[i] == entity.generation
    }

    /// Number of live entities.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterates over all live entities in index order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .enumerate()
            .filter(|(_, alive)| **alive)
            .map(|(i, _)| Entity {
                index: i as u32,
                generation: self.generations[i],
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_sequential_indices() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        let b = alloc.allocate();
        assert_eq!((a.index(), b.index()), (0, 1));
        assert_eq!(alloc.len(), 2);
    }

    #[test]
    fn reuses_slots_with_new_generation() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        assert!(alloc.deallocate(a));
        let b = alloc.allocate();
        assert_eq!(b.index(), a.index());
        assert_ne!(b.generation(), a.generation());
        assert!(!alloc.is_alive(a));
        assert!(alloc.is_alive(b));
    }

    #[test]
    fn double_free_is_rejected() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        assert!(alloc.deallocate(a));
        assert!(!alloc.deallocate(a));
        assert_eq!(alloc.len(), 0);
    }

    #[test]
    fn placeholder_is_never_alive() {
        let mut alloc = EntityAllocator::new();
        alloc.allocate();
        assert!(!alloc.is_alive(Entity::PLACEHOLDER));
    }

    #[test]
    fn bits_round_trip() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        alloc.deallocate(a);
        let b = alloc.allocate();
        assert_eq!(Entity::from_bits(b.to_bits()), b);
    }
}
