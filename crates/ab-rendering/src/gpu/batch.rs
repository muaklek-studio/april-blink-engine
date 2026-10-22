//! Ordering of draw items into instanced batches.
//!
//! Objects are written to the GPU in an order where items sharing pipeline, material and
//! mesh are adjacent, so each run becomes one `draw_indexed` with an instance range (the
//! shader indexes the object buffer with `instance_index`).

use crate::assets::{MaterialHandle, MeshHandle};
use crate::extract::DrawItem;
use std::ops::Range;

/// How a draw item is rendered; `None` from the classifier means "cannot be drawn".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DrawClass {
    pub skinned: bool,
    pub double_sided: bool,
}

/// A run of consecutive objects drawn with one instanced call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Batch {
    pub class: DrawClass,
    pub material: MaterialHandle,
    pub mesh: MeshHandle,
    pub instances: Range<u32>,
}

/// Object order and batches for one frame.
#[derive(Debug, Default)]
pub(crate) struct BatchPlan {
    /// Indices into the draw list, in GPU object order.
    pub order: Vec<usize>,
    /// Class of each entry in `order`.
    pub classes: Vec<DrawClass>,
    /// Batches for the camera pass (visible objects).
    pub forward: Vec<Batch>,
    /// Batches for shadow passes (shadow casters, visible or not). `material` is unused.
    pub shadow: Vec<Batch>,
}

impl BatchPlan {
    /// Layout: `[visible, sorted by (class, material, mesh, casters first)]
    /// [off-screen casters, sorted by (class, mesh)]`.
    pub fn build(&mut self, draws: &[DrawItem], classify: impl Fn(&DrawItem) -> Option<DrawClass>) {
        self.order.clear();
        self.classes.clear();
        self.forward.clear();
        self.shadow.clear();

        let mut visible: Vec<(DrawClass, usize)> = Vec::new();
        let mut offscreen: Vec<(DrawClass, usize)> = Vec::new();
        for (i, draw) in draws.iter().enumerate() {
            let Some(class) = classify(draw) else {
                continue;
            };
            if draw.in_view {
                visible.push((class, i));
            } else if draw.cast_shadows {
                offscreen.push((class, i));
            }
        }
        visible.sort_by_key(|&(class, i)| {
            let d = &draws[i];
            (class, d.material, d.mesh, !d.cast_shadows)
        });
        offscreen.sort_by_key(|&(class, i)| (class.skinned, draws[i].mesh));
        let visible_count = visible.len();
        for (class, i) in visible.into_iter().chain(offscreen) {
            self.order.push(i);
            self.classes.push(class);
        }

        for (slot, (&i, &class)) in self.order.iter().zip(&self.classes).enumerate() {
            let draw = &draws[i];
            let instance = slot as u32;
            if slot < visible_count {
                push_run(&mut self.forward, class, draw, instance, true);
            }
            if draw.cast_shadows {
                // Shadows ignore materials and culling mode; only the vertex path matters.
                let shadow_class = DrawClass {
                    skinned: class.skinned,
                    double_sided: false,
                };
                push_run(&mut self.shadow, shadow_class, draw, instance, false);
            }
        }
    }
}

fn push_run(
    out: &mut Vec<Batch>,
    class: DrawClass,
    draw: &DrawItem,
    instance: u32,
    by_material: bool,
) {
    let material = if by_material {
        draw.material
    } else {
        MaterialHandle(0)
    };
    match out.last_mut() {
        Some(batch)
            if batch.class == class
                && batch.material == material
                && batch.mesh == draw.mesh
                && batch.instances.end == instance =>
        {
            batch.instances.end += 1;
        }
        _ => out.push(Batch {
            class,
            material,
            mesh: draw.mesh,
            instances: instance..instance + 1,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_core::Affine3A;

    const STATIC: DrawClass = DrawClass {
        skinned: false,
        double_sided: false,
    };

    fn item(mesh: u32, material: u32, in_view: bool, cast_shadows: bool) -> DrawItem {
        DrawItem {
            model: Affine3A::IDENTITY,
            mesh: MeshHandle(mesh),
            material: MaterialHandle(material),
            cast_shadows,
            in_view,
            joints: None,
        }
    }

    fn batch(material: u32, mesh: u32, instances: Range<u32>) -> Batch {
        Batch {
            class: STATIC,
            material: MaterialHandle(material),
            mesh: MeshHandle(mesh),
            instances,
        }
    }

    #[test]
    fn groups_by_material_and_mesh_into_instanced_runs() {
        let draws = [
            item(1, 0, true, true),
            item(0, 0, true, true),
            item(1, 0, true, true),
            item(0, 0, true, false),
            item(0, 1, true, true),
        ];
        let mut plan = BatchPlan::default();
        plan.build(&draws, |_| Some(STATIC));
        assert_eq!(
            plan.forward,
            [batch(0, 0, 0..2), batch(0, 1, 2..4), batch(1, 0, 4..5)]
        );
        // Mesh 0's non-caster sits last in its run, so the casters stay contiguous; shadows
        // merge across materials when objects are adjacent.
        assert_eq!(
            plan.shadow,
            [batch(0, 0, 0..1), batch(0, 1, 2..4), batch(0, 0, 4..5)]
        );
    }

    #[test]
    fn offscreen_casters_only_go_to_shadow_batches() {
        let draws = [
            item(0, 0, false, true),
            item(0, 0, true, true),
            item(2, 0, false, false),
        ];
        let mut plan = BatchPlan::default();
        plan.build(&draws, |_| Some(STATIC));
        assert_eq!(plan.order, [1, 0]);
        assert_eq!(plan.forward, [batch(0, 0, 0..1)]);
        assert_eq!(plan.shadow, [batch(0, 0, 0..2)]);
    }

    #[test]
    fn classes_split_batches_and_undrawable_items_are_skipped() {
        let draws = [
            item(0, 0, true, true),
            item(0, 0, true, true),
            item(9, 0, true, true),
        ];
        let mut plan = BatchPlan::default();
        let skinned = DrawClass {
            skinned: true,
            double_sided: false,
        };
        plan.build(&draws, |d| {
            if d.mesh.0 == 9 {
                return None;
            }
            Some(if std::ptr::eq(d, &draws[0]) {
                skinned
            } else {
                STATIC
            })
        });
        assert_eq!(plan.order, [1, 0]);
        assert_eq!(plan.forward.len(), 2);
        assert_eq!(plan.forward[1].class, skinned);
    }
}
