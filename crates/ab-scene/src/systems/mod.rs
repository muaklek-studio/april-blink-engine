//! Scene systems. Each is a plain function over the [`World`](ab_ecs::World) so the engine
//! can schedule it wherever it belongs.

mod transform;

pub use transform::propagate_transforms;
