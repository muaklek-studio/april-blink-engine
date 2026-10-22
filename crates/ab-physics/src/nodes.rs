use crate::components::{BodyKind, CharacterBody3D, CollisionShape3D, PhysicsBody3D};
use crate::convert::Pose;
use crate::joint::{Joint3D, JointKind};
use crate::ragdoll::{self, plan_bones, Ragdoll, RagdollBone, SkeletonRagdoll};
use crate::shape::Shape;
use crate::world::PhysicsWorld;
use crate::{class, PhysicsError};
use ab_core::{Affine3A, Quat, Vec3};
use ab_scene::{NodeHandle, SceneError, SceneTree, Transform};
use std::collections::HashMap;

/// Physics node constructors and helpers on [`SceneTree`]
/// (`use ab_physics::PhysicsNodes;`).
pub trait PhysicsNodes {
    /// Creates a detached body node; its class (`RigidBody3D`, `Area3D`, ...) follows
    /// `body.kind`. Give it shapes with [`PhysicsNodes::add_collision_shape`].
    fn create_physics_body3d(&mut self, name: &str, body: PhysicsBody3D) -> NodeHandle;

    /// Creates a detached `CharacterBody3D`, moved with [`PhysicsNodes::move_and_slide`].
    fn create_character_body3d(&mut self, name: &str, character: CharacterBody3D) -> NodeHandle;

    /// Creates a detached `CollisionShape3D`.
    fn create_collision_shape3d(&mut self, name: &str, shape: Shape) -> NodeHandle;

    /// Creates a `CollisionShape3D` named `CollisionShape3D` and attaches it to `body`.
    fn add_collision_shape(
        &mut self,
        body: NodeHandle,
        shape: Shape,
    ) -> Result<NodeHandle, SceneError>;

    /// Moves a `CharacterBody3D` by its `velocity * delta`, sliding along floors, walls and
    /// ceilings, then updates `velocity` and the floor/wall/ceiling flags. Call it from
    /// `physics_process`. Returns whether the body hit anything.
    ///
    /// The body's first shape is the one swept. Before the body's first physics step the
    /// node just moves freely.
    fn move_and_slide(&mut self, node: NodeHandle, delta: f32) -> Result<bool, PhysicsError>;

    /// Creates a detached `Joint3D`. Place it where the bodies should be joined (and rotate
    /// it to aim hinge/twist axes, its X axis) before the first physics step with both
    /// bodies.
    fn create_joint3d(&mut self, name: &str, joint: Joint3D) -> NodeHandle;

    /// Creates a detached, empty `Ragdoll` node; put bodies and joints under it.
    fn create_ragdoll(&mut self, name: &str, ragdoll: Ragdoll) -> NodeHandle;

    /// Builds a ragdoll for a skeleton under `parent`: a capsule body per bone (head to the
    /// next bone), cone-twist joints between them, each tied to its bone. Inactive, it
    /// follows the animation; [`PhysicsNodes::set_ragdoll_active`] makes it go limp and move
    /// the bones (stop the model's animation first).
    fn create_skeleton_ragdoll(
        &mut self,
        parent: NodeHandle,
        name: &str,
        settings: &SkeletonRagdoll,
    ) -> Result<NodeHandle, PhysicsError>;

    /// Switches a ragdoll between limp (simulated) and kinematic.
    fn set_ragdoll_active(&mut self, ragdoll: NodeHandle, active: bool)
        -> Result<(), PhysicsError>;

    fn is_ragdoll_active(&self, ragdoll: NodeHandle) -> bool;

    /// The body nodes of a ragdoll.
    fn ragdoll_parts(&self, ragdoll: NodeHandle) -> Vec<NodeHandle>;

    /// Pushes the ragdoll part nearest to `at` (a world point) with `impulse` (N·s) applied
    /// at that point: a hit, a shot, an explosion. Returns the part pushed.
    fn apply_ragdoll_impulse(
        &mut self,
        ragdoll: NodeHandle,
        impulse: Vec3,
        at: Vec3,
    ) -> Option<NodeHandle>;
}

impl PhysicsNodes for SceneTree {
    fn create_physics_body3d(&mut self, name: &str, body: PhysicsBody3D) -> NodeHandle {
        self.spawn_node(name, body.kind.class_name(), (Transform::default(), body))
    }

    fn create_character_body3d(&mut self, name: &str, character: CharacterBody3D) -> NodeHandle {
        self.spawn_node(
            name,
            class::CHARACTER_BODY_3D,
            (Transform::default(), PhysicsBody3D::character(), character),
        )
    }

    fn create_collision_shape3d(&mut self, name: &str, shape: Shape) -> NodeHandle {
        self.spawn_node(
            name,
            class::COLLISION_SHAPE_3D,
            (Transform::default(), CollisionShape3D::new(shape)),
        )
    }

    fn add_collision_shape(
        &mut self,
        body: NodeHandle,
        shape: Shape,
    ) -> Result<NodeHandle, SceneError> {
        let node = self.create_collision_shape3d(class::COLLISION_SHAPE_3D, shape);
        if let Err(err) = self.add_child(body, node) {
            self.world_mut().despawn(node.entity());
            return Err(err);
        }
        Ok(node)
    }

    fn create_joint3d(&mut self, name: &str, joint: Joint3D) -> NodeHandle {
        self.spawn_node(name, class::JOINT_3D, (Transform::default(), joint))
    }

    fn create_ragdoll(&mut self, name: &str, ragdoll: Ragdoll) -> NodeHandle {
        self.spawn_node(name, class::RAGDOLL, (Transform::default(), ragdoll))
    }

    fn create_skeleton_ragdoll(
        &mut self,
        parent: NodeHandle,
        name: &str,
        settings: &SkeletonRagdoll,
    ) -> Result<NodeHandle, PhysicsError> {
        let globals: Vec<(NodeHandle, Affine3A)> = settings
            .bones
            .iter()
            .filter_map(|&bone| Some((bone, self.global_transform(bone)?)))
            .collect();
        let index: HashMap<NodeHandle, usize> = globals
            .iter()
            .enumerate()
            .map(|(i, (b, _))| (*b, i))
            .collect();
        // Nearest ancestor that is also a bone.
        let parents: Vec<Option<usize>> = globals
            .iter()
            .map(|(bone, _)| {
                std::iter::successors(self.parent(*bone), |&n| self.parent(n))
                    .find_map(|n| index.get(&n).copied())
            })
            .collect();
        // End bones continue their parent's proportions.
        let inner: Vec<f32> = plan_bones(&globals, &parents, 0.0)
            .iter()
            .map(|p| p.length())
            .filter(|&l| l > settings.min_bone_length)
            .collect();
        let fallback = if inner.is_empty() {
            0.15
        } else {
            0.6 * inner.iter().sum::<f32>() / inner.len() as f32
        };
        let plan = plan_bones(&globals, &parents, fallback);

        // Bodies: (bone index → part node, capsule radius, length).
        let mut parts: Vec<Option<NodeHandle>> = vec![None; plan.len()];
        let mut capsules = Vec::new();
        for (i, bone) in plan.iter().enumerate() {
            let length = bone.length();
            if length < settings.min_bone_length {
                continue;
            }
            let radius = (length * settings.radius)
                .clamp(settings.radius_range[0], settings.radius_range[1])
                .min(length * 0.5);
            let volume = std::f32::consts::PI * radius * radius * (length + radius * 4.0 / 3.0);
            capsules.push((i, radius, length, volume));
        }
        if capsules.is_empty() {
            return Err(PhysicsError::NoRagdollBones);
        }
        let total_volume: f32 = capsules.iter().map(|c| c.3).sum();
        let root = self.create_ragdoll(
            name,
            Ragdoll {
                active: settings.active,
            },
        );
        self.add_child(parent, root)?;
        let kind = if settings.active {
            BodyKind::Rigid
        } else {
            BodyKind::Kinematic
        };
        for &(i, radius, length, volume) in &capsules {
            let bone = plan[i];
            let direction = (bone.tail - bone.head).normalize_or(Vec3::Y);
            let global = Affine3A::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, direction),
                (bone.head + bone.tail) * 0.5,
            );
            let body = PhysicsBody3D::new(kind)
                .with_mass(settings.total_mass * volume / total_volume)
                .with_collision(settings.collision_layer, settings.collision_mask);
            let bone_name = self
                .name(bone.bone)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            let node = self.create_physics_body3d(&bone_name, body);
            self.add_collision_shape(
                node,
                Shape::Capsule {
                    radius,
                    height: length,
                },
            )?;
            self.add_child(root, node)?;
            self.set_global_transform(node, global)?;
            self.insert_component(
                node,
                RagdollBone {
                    bone: bone.bone,
                    offset: global.inverse() * globals[i].1,
                },
            )?;
            parts[i] = Some(node);
        }
        // Joints at each bone's head, to the nearest ancestor with a body.
        for &(i, ..) in &capsules {
            let parent_part =
                std::iter::successors(parents[i], |&p| parents[p]).find_map(|p| parts[p]);
            let (Some(parent_part), Some(part)) = (parent_part, parts[i]) else {
                continue;
            };
            let bone = plan[i];
            let direction = (bone.tail - bone.head).normalize_or(Vec3::Y);
            let joint = Joint3D::new(
                JointKind::ConeTwist {
                    swing: settings.swing_degrees.to_radians(),
                    twist: settings.twist_degrees.to_radians(),
                },
                parent_part,
                part,
            );
            let name = format!(
                "{}Joint",
                self.name(part).map(|n| n.to_owned()).unwrap_or_default()
            );
            let node = self.create_joint3d(&name, joint);
            self.add_child(root, node)?;
            self.set_global_transform(
                node,
                Affine3A::from_rotation_translation(
                    Quat::from_rotation_arc(Vec3::X, direction),
                    bone.head,
                ),
            )?;
        }
        Ok(root)
    }

    fn set_ragdoll_active(
        &mut self,
        ragdoll: NodeHandle,
        active: bool,
    ) -> Result<(), PhysicsError> {
        let mut component = self
            .component_mut::<Ragdoll>(ragdoll)
            .ok_or(PhysicsError::NotARagdoll(ragdoll))?;
        component.active = active;
        Ok(())
    }

    fn is_ragdoll_active(&self, ragdoll: NodeHandle) -> bool {
        self.component::<Ragdoll>(ragdoll).is_some_and(|r| r.active)
    }

    fn ragdoll_parts(&self, ragdoll: NodeHandle) -> Vec<NodeHandle> {
        ragdoll::descendants(self.world(), ragdoll.entity())
            .into_iter()
            .map(NodeHandle::from_entity)
            .filter(|&n| {
                self.component::<PhysicsBody3D>(n)
                    .is_some_and(|b| matches!(b.kind, BodyKind::Rigid | BodyKind::Kinematic))
            })
            .collect()
    }

    fn apply_ragdoll_impulse(
        &mut self,
        ragdoll: NodeHandle,
        impulse: Vec3,
        at: Vec3,
    ) -> Option<NodeHandle> {
        let nearest = self
            .ragdoll_parts(ragdoll)
            .into_iter()
            .filter_map(|n| Some((n, self.global_position(n)?.distance_squared(at))))
            .min_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        self.component_mut::<PhysicsBody3D>(nearest)?
            .apply_impulse_at(impulse, at);
        Some(nearest)
    }

    fn move_and_slide(&mut self, node: NodeHandle, delta: f32) -> Result<bool, PhysicsError> {
        let global = self.global_transform(node).ok_or(if self.is_valid(node) {
            SceneError::NotNode3D(node)
        } else {
            SceneError::InvalidNode(node)
        })?;
        let mut character = self
            .component::<CharacterBody3D>(node)
            .map(|c| c.clone())
            .ok_or(PhysicsError::NotACharacter(node))?;
        let (handle, mass) = self
            .component::<PhysicsBody3D>(node)
            .map(|b| (b.state.map(|s| s.handle), b.mass))
            .ok_or(PhysicsError::NotACharacter(node))?;

        let (_, rotation, position) = global.to_scale_rotation_translation();
        let slide = handle.and_then(|handle| {
            self.world()
                .resource_mut::<PhysicsWorld>()?
                .slide_character(handle, Pose { position, rotation }, &character, mass, delta)
        });
        let collided = match slide {
            Some(slide) => {
                character.velocity = slide.velocity;
                character.set_contacts(slide.floor, slide.wall, slide.ceiling);
                self.set_global_position(node, position + slide.translation)?;
                slide.collided
            }
            None => {
                character.set_contacts(None, false, false);
                self.set_global_position(node, position + character.velocity * delta)?;
                false
            }
        };
        if let Some(mut stored) = self.component_mut::<CharacterBody3D>(node) {
            *stored = character;
        }
        Ok(collided)
    }
}
