use crate::world::World;

/// A unit of logic run by a [`Schedule`](crate::Schedule).
pub trait System: 'static {
    fn name(&self) -> &str;
    fn run(&mut self, world: &mut World);
}

/// Conversion into a boxed [`System`]; implemented for every `FnMut(&mut World)`.
pub trait IntoSystem {
    fn into_system(self) -> Box<dyn System>;
}

impl<S: System> IntoSystem for S {
    fn into_system(self) -> Box<dyn System> {
        Box::new(self)
    }
}

struct FnSystem<F> {
    name: &'static str,
    f: F,
}

impl<F: FnMut(&mut World) + 'static> System for FnSystem<F> {
    fn name(&self) -> &str {
        self.name
    }

    fn run(&mut self, world: &mut World) {
        (self.f)(world)
    }
}

impl<F: FnMut(&mut World) + 'static> System for F {
    fn name(&self) -> &str {
        std::any::type_name::<F>()
    }

    fn run(&mut self, world: &mut World) {
        self(world)
    }
}

/// Wraps a closure with a readable name for logs and profiling.
pub fn named(name: &'static str, f: impl FnMut(&mut World) + 'static) -> impl System {
    FnSystem { name, f }
}
