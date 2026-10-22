//! Physics: queries, events, impulses, characters, ragdolls.

use super::Builder;
use crate::script_host::call::{Call, CallError};
use ab_physics::{
    BodyKind, PhysicsBody3D, PhysicsEvent, PhysicsNodes, PhysicsWorld, QueryOptions, Ragdoll,
};
use ab_scene::{NodeHandle, NodeInfo};

/// Reads the query options trailing a query's arguments, from `at`: mask, areas, exclusions.
fn options(c: &Call<'_>, at: usize) -> Result<QueryOptions, CallError> {
    let mut options = QueryOptions::new()
        .with_mask(c.u32(at)?)
        .with_areas(c.bool(at + 1)?);
    for node in c.nodes_from(at + 2) {
        options = options.excluding(node);
    }
    Ok(options)
}

fn physics<R>(c: &Call<'_>, f: impl FnOnce(&PhysicsWorld) -> R) -> Result<R, CallError> {
    let physics = c
        .tree
        .world()
        .resource::<PhysicsWorld>()
        .ok_or_else(|| CallError::failed("this scene has no physics"))?;
    Ok(f(&physics))
}

fn body<R>(
    c: &Call<'_>,
    node: NodeHandle,
    f: impl FnOnce(&mut PhysicsBody3D) -> R,
) -> Result<R, CallError> {
    let mut body = c
        .tree
        .component_mut::<PhysicsBody3D>(node)
        .ok_or_else(|| CallError::Failed(format!("{node} has no PhysicsBody3D")))?;
    Ok(f(&mut body))
}

pub(super) fn add(b: &mut Builder) {
    b.add("physics.cast_ray", |c| {
        let (origin, direction, max) = (c.vec3(0)?, c.vec3(1)?, c.f32(2)?);
        let options = options(c, 3)?;
        if let Some(hit) = physics(c, |p| p.cast_ray(origin, direction, max, &options))? {
            c.push_node(hit.node);
            c.push_vec3(hit.position);
            c.push_vec3(hit.normal);
            c.push_f32(hit.distance);
        }
        Ok(())
    });
    b.add("physics.line_of_sight", |c| {
        let (from, to) = (c.vec3(0)?, c.vec3(1)?);
        let options = options(c, 2)?;
        c.push_bool(physics(c, |p| p.has_line_of_sight(from, to, &options))?);
        Ok(())
    });
    b.add("physics.intersect_sphere", |c| {
        let (center, radius) = (c.vec3(0)?, c.f32(1)?);
        let options = options(c, 2)?;
        for node in physics(c, |p| p.intersect_sphere(center, radius, &options))? {
            c.push_node(node);
        }
        Ok(())
    });
    b.add("physics.events", |c| {
        let events = physics(c, |p| p.events().to_vec())?;
        for event in events {
            let (kind, a, b) = match event {
                PhysicsEvent::ContactStarted { a, b } => ("contact_started", a, b),
                PhysicsEvent::ContactEnded { a, b } => ("contact_ended", a, b),
                PhysicsEvent::BodyEntered { area, body } => ("body_entered", area, body),
                PhysicsEvent::BodyExited { area, body } => ("body_exited", area, body),
            };
            c.push_str(kind);
            c.push_node(a);
            c.push_node(b);
        }
        Ok(())
    });
    b.add("physics.bodies", |c| {
        let kind = match c.str(0)? {
            "static" => BodyKind::Static,
            "rigid" => BodyKind::Rigid,
            "kinematic" => BodyKind::Kinematic,
            "character" => BodyKind::Character,
            "area" => BodyKind::Area,
            other => {
                return Err(CallError::BadArguments(format!(
                    "unknown body kind `{other}`"
                )))
            }
        };
        let (Some(bodies), Some(infos)) = (
            c.tree.world().storage::<PhysicsBody3D>(),
            c.tree.world().storage::<NodeInfo>(),
        ) else {
            return Ok(());
        };
        for (entity, body) in bodies.iter() {
            if body.kind == kind && infos.get(entity).is_some_and(|i| i.in_tree) {
                c.push_node(NodeHandle::from_entity(entity));
            }
        }
        Ok(())
    });
    b.add("physics.gravity", |c| {
        c.push_vec3(physics(c, |p| p.gravity)?);
        Ok(())
    });

    b.add("body.apply_impulse", |c| {
        let (node, impulse) = (c.node(0)?, c.vec3(1)?);
        body(c, node, |b| b.apply_central_impulse(impulse))
    });
    b.add("body.apply_impulse_at", |c| {
        let (node, impulse, at) = (c.node(0)?, c.vec3(1)?, c.vec3(2)?);
        body(c, node, |b| b.apply_impulse_at(impulse, at))
    });
    b.add("body.apply_torque_impulse", |c| {
        let (node, impulse) = (c.node(0)?, c.vec3(1)?);
        body(c, node, |b| b.apply_torque_impulse(impulse))
    });
    b.add("body.apply_force", |c| {
        let (node, force) = (c.node(0)?, c.vec3(1)?);
        body(c, node, |b| b.apply_central_force(force))
    });
    b.add("body.apply_torque", |c| {
        let (node, torque) = (c.node(0)?, c.vec3(1)?);
        body(c, node, |b| b.apply_torque(torque))
    });
    b.add("body.colliding_bodies", |c| {
        let node = c.node(0)?;
        for other in physics(c, |p| p.colliding_bodies(node))? {
            c.push_node(other);
        }
        Ok(())
    });
    b.add("body.overlapping_bodies", |c| {
        let node = c.node(0)?;
        for other in physics(c, |p| p.overlapping_bodies(node))? {
            c.push_node(other);
        }
        Ok(())
    });
    b.add("character.move_and_slide", |c| {
        let (node, delta) = (c.node(0)?, c.f32(1)?);
        let collided = c
            .tree
            .move_and_slide(node, delta)
            .map_err(CallError::failed)?;
        c.push_bool(collided);
        Ok(())
    });

    b.add("ragdoll.of", |c| {
        let node = c.node(0)?;
        let ragdoll = std::iter::successors(Some(node), |&n| c.tree.parent(n))
            .find(|&n| c.tree.component::<Ragdoll>(n).is_some());
        c.push_opt_node(ragdoll);
        Ok(())
    });
    b.add("ragdoll.go_limp", |c| {
        let node = c.node(0)?;
        crate::ragdoll::go_limp(c.tree, node, None).map_err(CallError::failed)
    });
    b.add("ragdoll.is_active", |c| {
        let node = c.node(0)?;
        c.push_bool(c.tree.is_ragdoll_active(node));
        Ok(())
    });
}
