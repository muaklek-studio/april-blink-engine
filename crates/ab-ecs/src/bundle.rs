use crate::component::Component;
use crate::entity::Entity;
use crate::world::World;

/// A set of components inserted together, e.g. by [`World::spawn`].
///
/// Implemented for tuples of up to 12 components. Use `(component,)` for a single one.
pub trait Bundle: 'static {
    /// Inserts every component. `entity` must be alive.
    fn insert_into(self, world: &mut World, entity: Entity);
}

impl Bundle for () {
    fn insert_into(self, _: &mut World, _: Entity) {}
}

macro_rules! impl_bundle_tuple {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: Component),+> Bundle for ($($name,)+) {
            fn insert_into(self, world: &mut World, entity: Entity) {
                let ($($name,)+) = self;
                $(
                    world.insert(entity, $name).expect("bundle target entity must be alive");
                )+
            }
        }
    };
}

impl_bundle_tuple!(A);
impl_bundle_tuple!(A, B);
impl_bundle_tuple!(A, B, C);
impl_bundle_tuple!(A, B, C, D);
impl_bundle_tuple!(A, B, C, D, E);
impl_bundle_tuple!(A, B, C, D, E, F);
impl_bundle_tuple!(A, B, C, D, E, F, G);
impl_bundle_tuple!(A, B, C, D, E, F, G, H);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J, K);
impl_bundle_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
