use crate::component::Component;
use crate::entity::Entity;
use crate::sparse_set::SparseSet;
use crate::world::World;
use std::cell::{Ref, RefMut};
use std::fmt;
use std::marker::PhantomData;

/// Something that can be fetched per entity by a query: `&T`, `&mut T`, `Option<&T>`,
/// `Option<&mut T>`, [`Entity`], or a tuple of those (up to 8).
///
/// A query must contain at least one required component (`&T` / `&mut T`); the smallest
/// matching storage drives iteration.
pub trait QueryData {
    /// Borrow guards held for the lifetime of the [`Query`].
    type State<'w>;
    /// Value yielded per entity.
    type Item<'q>;

    /// Borrows the storages. `None` means the query cannot match anything (a required
    /// component type was never inserted). Panics on a borrow conflict.
    fn borrow_state(world: &World) -> Option<Self::State<'_>>;

    /// Entities of the smallest required storage.
    fn driver<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]>;

    /// Fetches the item for `entity`, or `None` if it does not match.
    ///
    /// # Safety
    /// The returned item is not tied to the borrow of `state`. The caller must keep `state`
    /// alive and unmoved for `'q`, must not fetch the same entity again while a previous item
    /// for it is alive, and must not structurally modify the storages meanwhile.
    unsafe fn fetch<'q>(state: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>>;
}

impl<T: Component> QueryData for &T {
    type State<'w> = Ref<'w, SparseSet<T>>;
    type Item<'q> = &'q T;

    fn borrow_state(world: &World) -> Option<Self::State<'_>> {
        world.storage::<T>()
    }

    fn driver<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        Some(state.entities())
    }

    unsafe fn fetch<'q>(state: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>> {
        let value: *const T = state.get(entity)?;
        // SAFETY: the storage stays borrowed (and therefore unmodified) for `'q` per the
        // caller contract, and shared references may alias.
        Some(unsafe { &*value })
    }
}

impl<T: Component> QueryData for &mut T {
    type State<'w> = RefMut<'w, SparseSet<T>>;
    type Item<'q> = &'q mut T;

    fn borrow_state(world: &World) -> Option<Self::State<'_>> {
        world.storage_mut::<T>()
    }

    fn driver<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        Some(state.entities())
    }

    unsafe fn fetch<'q>(state: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>> {
        let value = state.get_ptr(entity)?;
        // SAFETY: the storage is exclusively borrowed for `'q`, and the caller never fetches
        // the same entity twice, so each `&mut T` handed out is unique.
        Some(unsafe { &mut *value })
    }
}

impl<T: Component> QueryData for Option<&T> {
    type State<'w> = Option<Ref<'w, SparseSet<T>>>;
    type Item<'q> = Option<&'q T>;

    fn borrow_state(world: &World) -> Option<Self::State<'_>> {
        Some(world.storage::<T>())
    }

    fn driver<'s>(_: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        None
    }

    unsafe fn fetch<'q>(state: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>> {
        Some(match state {
            // SAFETY: forwarded caller contract.
            Some(set) => unsafe { <&T as QueryData>::fetch(set, entity) },
            None => None,
        })
    }
}

impl<T: Component> QueryData for Option<&mut T> {
    type State<'w> = Option<RefMut<'w, SparseSet<T>>>;
    type Item<'q> = Option<&'q mut T>;

    fn borrow_state(world: &World) -> Option<Self::State<'_>> {
        Some(world.storage_mut::<T>())
    }

    fn driver<'s>(_: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        None
    }

    unsafe fn fetch<'q>(state: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>> {
        Some(match state {
            // SAFETY: forwarded caller contract.
            Some(set) => unsafe { <&mut T as QueryData>::fetch(set, entity) },
            None => None,
        })
    }
}

impl QueryData for Entity {
    type State<'w> = ();
    type Item<'q> = Entity;

    fn borrow_state(_: &World) -> Option<Self::State<'_>> {
        Some(())
    }

    fn driver<'s>(_: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        None
    }

    unsafe fn fetch<'q>(_: &mut Self::State<'_>, entity: Entity) -> Option<Self::Item<'q>> {
        Some(entity)
    }
}

macro_rules! impl_query_data_tuple {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: QueryData),+> QueryData for ($($name,)+) {
            type State<'w> = ($($name::State<'w>,)+);
            type Item<'q> = ($($name::Item<'q>,)+);

            fn borrow_state(world: &World) -> Option<Self::State<'_>> {
                Some(($($name::borrow_state(world)?,)+))
            }

            fn driver<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
                let ($($name,)+) = state;
                let mut best: Option<&'s [Entity]> = None;
                $(
                    if let Some(candidate) = $name::driver($name) {
                        if best.is_none_or(|b| candidate.len() < b.len()) {
                            best = Some(candidate);
                        }
                    }
                )+
                best
            }

            unsafe fn fetch<'q>(
                state: &mut Self::State<'_>,
                entity: Entity,
            ) -> Option<Self::Item<'q>> {
                let ($($name,)+) = state;
                // SAFETY: forwarded caller contract.
                Some(($(unsafe { $name::fetch($name, entity)? },)+))
            }
        }
    };
}

impl_query_data_tuple!(A);
impl_query_data_tuple!(A, B);
impl_query_data_tuple!(A, B, C);
impl_query_data_tuple!(A, B, C, D);
impl_query_data_tuple!(A, B, C, D, E);
impl_query_data_tuple!(A, B, C, D, E, F);
impl_query_data_tuple!(A, B, C, D, E, F, G);
impl_query_data_tuple!(A, B, C, D, E, F, G, H);

/// Narrows a query without fetching data: [`With<T>`], [`Without<T>`], or tuples of them.
pub trait QueryFilter {
    type State<'w>;

    fn borrow_state(world: &World) -> Self::State<'_>;
    fn matches(state: &Self::State<'_>, entity: Entity) -> bool;
}

/// Only entities that have component `T`.
pub struct With<T>(PhantomData<fn() -> T>);

/// Only entities that do not have component `T`.
pub struct Without<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for With<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "With<{}>", std::any::type_name::<T>())
    }
}

impl<T> fmt::Debug for Without<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Without<{}>", std::any::type_name::<T>())
    }
}

impl QueryFilter for () {
    type State<'w> = ();

    fn borrow_state(_: &World) -> Self::State<'_> {}

    fn matches(_: &Self::State<'_>, _: Entity) -> bool {
        true
    }
}

impl<T: Component> QueryFilter for With<T> {
    type State<'w> = Option<Ref<'w, SparseSet<T>>>;

    fn borrow_state(world: &World) -> Self::State<'_> {
        world.storage::<T>()
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        state.as_ref().is_some_and(|s| s.contains(entity))
    }
}

impl<T: Component> QueryFilter for Without<T> {
    type State<'w> = Option<Ref<'w, SparseSet<T>>>;

    fn borrow_state(world: &World) -> Self::State<'_> {
        world.storage::<T>()
    }

    fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
        !state.as_ref().is_some_and(|s| s.contains(entity))
    }
}

macro_rules! impl_query_filter_tuple {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {
            type State<'w> = ($($name::State<'w>,)+);

            fn borrow_state(world: &World) -> Self::State<'_> {
                ($($name::borrow_state(world),)+)
            }

            fn matches(state: &Self::State<'_>, entity: Entity) -> bool {
                let ($($name,)+) = state;
                $($name::matches($name, entity))&&+
            }
        }
    };
}

impl_query_filter_tuple!(A);
impl_query_filter_tuple!(A, B);
impl_query_filter_tuple!(A, B, C);
impl_query_filter_tuple!(A, B, C, D);

/// Borrowed view over all entities matching `Q` and `F`.
///
/// Holds the storage borrows until dropped. Created by [`World::query`].
pub struct Query<'w, Q: QueryData, F: QueryFilter = ()> {
    state: Option<Q::State<'w>>,
    filter: F::State<'w>,
}

impl<'w, Q: QueryData, F: QueryFilter> Query<'w, Q, F> {
    pub(crate) fn new(world: &'w World) -> Self {
        let state = Q::borrow_state(world);
        if let Some(state) = &state {
            assert!(
                Q::driver(state).is_some(),
                "query `{}` needs at least one required component (`&T` or `&mut T`)",
                std::any::type_name::<Q>()
            );
        }
        Self {
            state,
            filter: F::borrow_state(world),
        }
    }

    pub fn iter(&mut self) -> QueryIter<'_, 'w, Q, F> {
        let (entities, len) = match &self.state {
            Some(state) => {
                let driver = Q::driver(state).unwrap_or(&[]);
                (driver.as_ptr(), driver.len())
            }
            None => (std::ptr::NonNull::dangling().as_ptr() as *const Entity, 0),
        };
        QueryIter {
            state: self.state.as_mut(),
            filter: &self.filter,
            entities,
            len,
            pos: 0,
        }
    }

    /// Fetches the item for a single entity, if it matches.
    pub fn get(&mut self, entity: Entity) -> Option<Q::Item<'_>> {
        let state = self.state.as_mut()?;
        if !F::matches(&self.filter, entity) {
            return None;
        }
        // SAFETY: the returned item borrows `self` mutably, so no other item can coexist and
        // the state outlives it.
        unsafe { Q::fetch(state, entity) }
    }

    /// Number of matching entities (walks the query).
    pub fn count(&mut self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&mut self) -> bool {
        self.iter().next().is_none()
    }
}

impl<'q, 'w, Q: QueryData, F: QueryFilter> IntoIterator for &'q mut Query<'w, Q, F> {
    type Item = Q::Item<'q>;
    type IntoIter = QueryIter<'q, 'w, Q, F>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<Q: QueryData, F: QueryFilter> fmt::Debug for Query<'_, Q, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("data", &std::any::type_name::<Q>())
            .finish_non_exhaustive()
    }
}

/// Iterator returned by [`Query::iter`].
pub struct QueryIter<'q, 'w, Q: QueryData, F: QueryFilter> {
    state: Option<&'q mut Q::State<'w>>,
    filter: &'q F::State<'w>,
    // Raw view of the driving storage's entity list. The storages are borrowed by `state` for
    // `'q`, so the list cannot be reallocated or modified while iterating.
    entities: *const Entity,
    len: usize,
    pos: usize,
}

impl<'q, Q: QueryData, F: QueryFilter> Iterator for QueryIter<'q, '_, Q, F> {
    type Item = Q::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        let state = self.state.as_mut()?;
        while self.pos < self.len {
            // SAFETY: `pos < len` and the entity list outlives the iterator (see field docs).
            let entity = unsafe { *self.entities.add(self.pos) };
            self.pos += 1;
            if !F::matches(self.filter, entity) {
                continue;
            }
            // SAFETY: `state` is borrowed for `'q`; each driver entity is visited exactly once,
            // so mutable items never alias; nothing modifies the storages structurally.
            if let Some(item) = unsafe { Q::fetch(state, entity) } {
                return Some(item);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.len - self.pos))
    }
}

impl<Q: QueryData, F: QueryFilter> fmt::Debug for QueryIter<'_, '_, Q, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryIter")
            .field("remaining", &(self.len - self.pos))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Pos(f32);
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Vel(f32);
    struct Frozen;

    #[test]
    fn single_component_query() {
        let mut world = World::new();
        world.spawn((Pos(1.0),));
        world.spawn((Pos(2.0),));
        world.spawn((Vel(3.0),));
        let mut sum = 0.0;
        for pos in world.query::<&Pos>().iter() {
            sum += pos.0;
        }
        assert_eq!(sum, 3.0);
    }

    #[test]
    fn multi_component_query_only_matches_entities_with_all() {
        let mut world = World::new();
        let a = world.spawn((Pos(0.0), Vel(1.0)));
        let b = world.spawn((Pos(10.0),));
        let c = world.spawn((Pos(20.0), Vel(-1.0)));

        for (pos, vel) in world.query::<(&mut Pos, &Vel)>().iter() {
            pos.0 += vel.0;
        }

        assert_eq!(world.get_copied::<Pos>(a), Some(Pos(1.0)));
        assert_eq!(world.get_copied::<Pos>(b), Some(Pos(10.0)));
        assert_eq!(world.get_copied::<Pos>(c), Some(Pos(19.0)));
    }

    #[test]
    fn optional_and_entity_items() {
        let mut world = World::new();
        let a = world.spawn((Pos(0.0), Vel(1.0)));
        let b = world.spawn((Pos(0.0),));
        let mut seen: Vec<_> = world
            .query::<(Entity, &Pos, Option<&Vel>)>()
            .iter()
            .map(|(e, _, v)| (e, v.copied()))
            .collect();
        seen.sort_by_key(|(e, _)| *e);
        assert_eq!(seen, vec![(a, Some(Vel(1.0))), (b, None)]);
    }

    #[test]
    fn filters() {
        let mut world = World::new();
        let moving = world.spawn((Pos(0.0),));
        let frozen = world.spawn((Pos(0.0), Frozen));

        let free: Vec<_> = world
            .query_filtered::<(Entity, &Pos), Without<Frozen>>()
            .iter()
            .map(|(e, _)| e)
            .collect();
        assert_eq!(free, vec![moving]);

        let only_frozen: Vec<_> = world
            .query_filtered::<(Entity, &Pos), With<Frozen>>()
            .iter()
            .map(|(e, _)| e)
            .collect();
        assert_eq!(only_frozen, vec![frozen]);
    }

    #[test]
    fn query_on_missing_component_type_is_empty() {
        let mut world = World::new();
        world.spawn((Pos(0.0),));
        assert_eq!(world.query::<(&Pos, &Vel)>().count(), 0);
    }

    #[test]
    fn query_get_single_entity() {
        let mut world = World::new();
        let a = world.spawn((Pos(1.0), Vel(2.0)));
        let b = world.spawn((Pos(1.0),));
        let mut query = world.query::<(&Pos, &mut Vel)>();
        query.get(a).unwrap().1 .0 = 5.0;
        assert!(query.get(b).is_none());
        drop(query);
        assert_eq!(world.get_copied::<Vel>(a), Some(Vel(5.0)));
    }

    #[test]
    fn iteration_is_driven_by_smallest_storage() {
        let mut world = World::new();
        for i in 0..100 {
            world.spawn((Pos(i as f32),));
        }
        let tagged = world.spawn((Pos(-1.0), Vel(0.0)));
        let iter_len = world.query::<(&Pos, &Vel)>().iter().size_hint().1;
        assert_eq!(iter_len, Some(1));
        assert_eq!(
            world
                .query::<(Entity, &Pos, &Vel)>()
                .iter()
                .next()
                .unwrap()
                .0,
            tagged
        );
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn aliasing_mutable_query_panics() {
        let mut world = World::new();
        world.spawn((Pos(0.0),));
        let _q = world.query::<(&mut Pos, &mut Pos)>();
    }

    #[test]
    #[should_panic(expected = "needs at least one required component")]
    fn query_without_required_component_panics() {
        let mut world = World::new();
        world.spawn((Pos(0.0),));
        let _q = world.query::<(Entity, Option<&Pos>)>();
    }
}
