use crate::bundle::Bundle;
use crate::component::Component;
use crate::entity::Entity;
use crate::world::World;
use std::fmt;

type Command = Box<dyn FnOnce(&mut World)>;

/// Queue of deferred world changes.
///
/// Use it when the world is borrowed (inside a query loop or a script callback) and apply the
/// queue later with [`Commands::apply`] or [`World::flush`]. Commands run in insertion order;
/// commands targeting entities that died in the meantime are ignored.
#[derive(Default)]
pub struct Commands {
    queue: Vec<Command>,
}

impl Commands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn<B: Bundle>(&mut self, bundle: B) {
        self.add(move |world: &mut World| {
            world.spawn(bundle);
        });
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.add(move |world: &mut World| {
            world.despawn(entity);
        });
    }

    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
        self.add(move |world: &mut World| {
            let _ = world.insert(entity, component);
        });
    }

    pub fn remove<T: Component>(&mut self, entity: Entity) {
        self.add(move |world: &mut World| {
            world.remove::<T>(entity);
        });
    }

    /// Queues an arbitrary world mutation.
    pub fn add(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.queue.push(Box::new(command));
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Applies and clears the queue.
    pub fn apply(&mut self, world: &mut World) {
        for command in self.queue.drain(..) {
            command(world);
        }
    }
}

impl fmt::Debug for Commands {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Commands")
            .field("len", &self.queue.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Hp(i32);

    #[test]
    fn applies_in_order() {
        let mut world = World::new();
        let e = world.spawn_empty();
        let mut commands = Commands::new();
        commands.insert(e, Hp(1));
        commands.insert(e, Hp(2));
        commands.spawn((Hp(9),));
        assert_eq!(commands.len(), 3);
        commands.apply(&mut world);
        assert!(commands.is_empty());
        assert_eq!(*world.get::<Hp>(e).unwrap(), Hp(2));
        assert_eq!(world.entity_count(), 2);
    }

    #[test]
    fn commands_on_dead_entities_are_ignored() {
        let mut world = World::new();
        let e = world.spawn((Hp(1),));
        let mut commands = Commands::new();
        commands.despawn(e);
        commands.insert(e, Hp(5));
        commands.remove::<Hp>(e);
        commands.apply(&mut world);
        assert!(!world.is_alive(e));
    }
}
