use crate::components::CharacterBody3D;
use crate::convert::Pose;
use crate::world::PhysicsWorld;
use ab_core::Vec3;
use rapier3d::control::{
    CharacterAutostep, CharacterCollision, CharacterLength, KinematicCharacterController,
};
use rapier3d::parry::query::contact;
use rapier3d::prelude::{
    Group, InteractionGroups, InteractionTestMode, QueryFilter, RigidBodyHandle, Shape,
};

/// Outcome of one character move.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Slide {
    pub translation: Vec3,
    pub velocity: Vec3,
    pub floor: Option<Vec3>,
    pub wall: bool,
    pub ceiling: bool,
    pub collided: bool,
}

/// Gap kept between the character and its surroundings, in metres.
const SKIN: f32 = 0.01;

impl PhysicsWorld {
    /// Moves a character's first shape by `velocity * delta` from `pose`, sliding along what
    /// it hits. Returns `None` if the body or its shape is not in the simulation yet.
    pub(crate) fn slide_character(
        &mut self,
        body: RigidBodyHandle,
        pose: Pose,
        character: &CharacterBody3D,
        mass: f32,
        delta: f32,
    ) -> Option<Slide> {
        let collider = self
            .colliders
            .get(*self.bodies.get(body)?.colliders().first()?)?;
        let shape = collider.shared_shape().clone();
        let shape_pose =
            pose.to_rapier() * collider.position_wrt_parent().copied().unwrap_or_default();
        let filter = QueryFilter::new()
            .exclude_rigid_body(body)
            .exclude_sensors()
            // Like Godot, a character is stopped by what its mask selects.
            .groups(InteractionGroups::new(
                Group::ALL,
                Group::from_bits_retain(crate::world::collider_layers(collider).1),
                InteractionTestMode::And,
            ));

        let up = character.up.try_normalize().unwrap_or(Vec3::Y);
        let snapping = character.floor_snap_length > 0.0
            && character.is_on_floor()
            && character.velocity.dot(up) <= 0.0;
        let controller = KinematicCharacterController {
            up,
            offset: CharacterLength::Absolute(SKIN),
            slide: true,
            autostep: (character.max_step_height > 0.0).then_some(CharacterAutostep {
                max_height: CharacterLength::Absolute(character.max_step_height),
                min_width: CharacterLength::Absolute(0.05),
                include_dynamic_bodies: false,
            }),
            max_slope_climb_angle: character.floor_max_angle,
            min_slope_slide_angle: character.floor_max_angle,
            snap_to_ground: snapping
                .then_some(CharacterLength::Absolute(character.floor_snap_length)),
            ..KinematicCharacterController::default()
        };

        let mut motion = character.velocity * delta;
        if snapping {
            // Snapping keeps the character on the floor, so the step's push into it adds
            // nothing. Leaving it out also keeps Rapier's slope handling from dropping the
            // whole step when a flat floor is hit with a downward motion (its float noise
            // reads as slipping).
            motion -= up * motion.dot(up);
        }
        let mut collisions = Vec::new();
        let movement = controller.move_shape(
            delta,
            &self.query_pipeline(filter),
            shape.as_ref(),
            &shape_pose,
            motion,
            |collision| collisions.push(collision),
        );
        if character.push_rigid_bodies {
            let mut queries = self.broad_phase.as_query_pipeline_mut(
                self.narrow_phase.query_dispatcher(),
                &mut self.bodies,
                &mut self.colliders,
                filter,
            );
            controller.solve_character_collision_impulses(
                delta,
                &mut queries,
                shape.as_ref(),
                mass,
                &collisions,
            );
        }

        // Slide the velocity along what was hit, like Godot's `move_and_slide`: floors and
        // ceilings only cancel motion along `up`, walls only horizontal motion into them, so
        // slightly tilted contact normals never make the body hop or drift.
        let max_cos = character.floor_max_angle.cos();
        let mut slide = Slide {
            translation: movement.translation,
            velocity: character.velocity,
            floor: None,
            wall: false,
            ceiling: false,
            collided: !collisions.is_empty(),
        };
        for collision in &collisions {
            let normal = self.hit_normal(shape.as_ref(), collision);
            let facing = normal.dot(up);
            let along_up = slide.velocity.dot(up);
            if facing >= max_cos - 1e-4 {
                slide.floor = Some(normal);
                slide.velocity -= up * along_up.min(0.0);
            } else if facing <= -max_cos + 1e-4 {
                slide.ceiling = true;
                slide.velocity -= up * along_up.max(0.0);
            } else {
                slide.wall = true;
                if let Some(wall) = (normal - up * facing).try_normalize() {
                    slide.velocity -= wall * slide.velocity.dot(wall).min(0.0);
                }
            }
        }
        if movement.grounded {
            slide.floor.get_or_insert(up);
            let along_up = slide.velocity.dot(up);
            slide.velocity -= up * along_up.min(0.0);
        }
        Some(slide)
    }

    /// The outward normal of what the character hit. Parry's shape-cast normal degrades
    /// towards the cast direction when the cast grazes a surface the character already
    /// touches (sliding along a wall), so it is recomputed from the closest points.
    fn hit_normal(&self, shape: &dyn Shape, collision: &CharacterCollision) -> Vec3 {
        self.colliders
            .get(collision.handle)
            .and_then(|obstacle| {
                contact(
                    obstacle.position(),
                    obstacle.shape(),
                    &collision.character_pos,
                    shape,
                    SKIN * 4.0,
                )
                .ok()
                .flatten()
            })
            .map_or(collision.hit.normal1, |contact| contact.normal1)
    }
}
