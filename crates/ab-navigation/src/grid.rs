use ab_core::{jobs, Vec2, Vec3};
use ab_physics::{PhysicsWorld, QueryOptions};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt;
use tracing::debug;

/// Cells baked per worker job (each costs a few ray casts and sphere queries).
const CELLS_PER_JOB: usize = 64;

/// Grid coordinates (x along world X, z along world Z).
pub type Cell = (i32, i32);

/// How [`NavGrid::bake`] samples the level.
#[derive(Debug, Clone)]
pub struct NavBakeSettings {
    /// Corner of the baked area with the lowest X, Y and Z. Rays start at `max.y`.
    pub min: Vec3,
    pub max: Vec3,
    /// Cell edge in metres.
    pub cell_size: f32,
    /// Clearance around the agent's centre line.
    pub agent_radius: f32,
    pub agent_height: f32,
    /// Largest height difference between neighbouring cells the agent can step.
    pub max_step: f32,
    /// Steepest walkable floor, as the minimum Y of its normal (0.7 ≈ 45°).
    pub min_floor_normal_y: f32,
    /// What counts as floor and obstacle (exclude the agents themselves here).
    pub query: QueryOptions,
}

impl Default for NavBakeSettings {
    fn default() -> Self {
        Self {
            min: Vec3::new(-20.0, -1.0, -20.0),
            max: Vec3::new(20.0, 10.0, 20.0),
            cell_size: 0.25,
            agent_radius: 0.35,
            agent_height: 1.8,
            max_step: 0.3,
            min_floor_normal_y: 0.7,
            query: QueryOptions::new(),
        }
    }
}

/// Walkable cells and their floor heights. Also an ECS resource for agents to share.
#[derive(Clone)]
pub struct NavGrid {
    origin: Vec2,
    cell_size: f32,
    width: i32,
    depth: i32,
    max_step: f32,
    /// Floor height per cell; NaN where not walkable.
    heights: Vec<f32>,
}

impl NavGrid {
    /// Most cells a grid may have (16 M: 64 MB of heights).
    pub const MAX_CELLS: u64 = 1 << 24;

    /// A grid covering `min..max` on XZ with every cell blocked.
    ///
    /// Grids are capped at [`NavGrid::MAX_CELLS`]: a huge area with tiny cells gets larger
    /// cells instead of exhausting memory.
    pub fn new(min: Vec2, max: Vec2, cell_size: f32) -> Self {
        let finite = |v: Vec2| if v.is_finite() { v } else { Vec2::ZERO };
        let (min, max) = (finite(min), finite(max));
        let mut cell_size = if cell_size.is_finite() {
            cell_size.max(0.01)
        } else {
            1.0
        };
        let extent = (max - min).max(Vec2::splat(cell_size));
        let cells = |size: f32| {
            let w = (extent.x / size).ceil().max(1.0) as u64;
            let d = (extent.y / size).ceil().max(1.0) as u64;
            (w, d)
        };
        let (w, d) = cells(cell_size);
        if w.saturating_mul(d) > Self::MAX_CELLS {
            let grown = cell_size * ((w * d) as f32 / Self::MAX_CELLS as f32).sqrt() * 1.01;
            tracing::warn!(
                requested = cell_size,
                used = grown,
                "navigation grid too fine for its area; using larger cells"
            );
            cell_size = grown;
        }
        let (w, d) = cells(cell_size);
        let (width, depth) = (w as i32, d as i32);
        Self {
            origin: min,
            cell_size,
            width,
            depth,
            max_step: f32::INFINITY,
            heights: vec![f32::NAN; (width * depth) as usize],
        }
    }

    /// Samples the physics world, in parallel over cells on the [`ab_core::jobs`] workers.
    pub fn bake(physics: &PhysicsWorld, settings: &NavBakeSettings) -> Self {
        let mut grid = Self::new(
            Vec2::new(settings.min.x, settings.min.z),
            Vec2::new(settings.max.x, settings.max.z),
            settings.cell_size,
        );
        grid.max_step = settings.max_step;
        let drop = settings.max.y - settings.min.y;
        let radius = settings.agent_radius.max(0.01);
        // Spheres stacked from just above the floor to the top of the agent.
        let clearance = (settings.agent_height - radius).max(radius);
        let samples = ((clearance - radius) / radius).ceil().max(0.0) as usize + 1;
        let floor_at = |center: Vec2| -> Option<f32> {
            let top = Vec3::new(center.x, settings.max.y, center.y);
            let hit = physics.cast_ray(top, Vec3::NEG_Y, drop, &settings.query)?;
            if hit.normal.y < settings.min_floor_normal_y {
                return None;
            }
            let floor = hit.position.y;
            // The agent needs support around it too: rules out thin wall tops and the
            // very edge of drops.
            let support = radius * 0.7;
            let supported = [
                (support, 0.0),
                (-support, 0.0),
                (0.0, support),
                (0.0, -support),
            ]
            .into_iter()
            .all(|(dx, dz)| {
                let from = Vec3::new(center.x + dx, settings.max.y, center.y + dz);
                physics
                    .cast_ray(from, Vec3::NEG_Y, drop, &settings.query)
                    .is_some_and(|h| (h.position.y - floor).abs() <= settings.max_step)
            });
            if !supported {
                return None;
            }
            let blocked = (0..samples).any(|i| {
                let y = floor + radius + 0.05 + i as f32 * radius;
                let y = y.min(floor + clearance);
                !physics
                    .intersect_sphere(Vec3::new(center.x, y, center.y), radius, &settings.query)
                    .is_empty()
            });
            (!blocked).then_some(floor)
        };
        // Cells are independent scene queries: sample them on the worker threads.
        let mut heights = std::mem::take(&mut grid.heights);
        let width = grid.width.max(1) as usize;
        jobs::par_for_each_indexed_mut(&mut heights, CELLS_PER_JOB, |index, height| {
            let cell = ((index % width) as i32, (index / width) as i32);
            *height = floor_at(grid.cell_center_xz(cell)).unwrap_or(f32::NAN);
        });
        grid.heights = heights;
        let walkable = grid.heights.iter().filter(|h| !h.is_nan()).count();
        debug!(
            width = grid.width,
            depth = grid.depth,
            walkable,
            "navigation grid baked"
        );
        grid
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn depth(&self) -> i32 {
        self.depth
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    fn index(&self, (x, z): Cell) -> usize {
        (z * self.width + x) as usize
    }

    fn in_bounds(&self, (x, z): Cell) -> bool {
        x >= 0 && z >= 0 && x < self.width && z < self.depth
    }

    /// Marks a cell walkable at floor `height` (or blocked with `None`), e.g. for doors.
    pub fn set_walkable(&mut self, cell: Cell, height: Option<f32>) {
        if self.in_bounds(cell) {
            let index = self.index(cell);
            self.heights[index] = height.unwrap_or(f32::NAN);
        }
    }

    pub fn is_walkable(&self, cell: Cell) -> bool {
        self.height(cell).is_some()
    }

    /// Floor height of a walkable cell.
    pub fn height(&self, cell: Cell) -> Option<f32> {
        if !self.in_bounds(cell) {
            return None;
        }
        let height = self.heights[self.index(cell)];
        (!height.is_nan()).then_some(height)
    }

    pub fn walkable_count(&self) -> usize {
        self.heights.iter().filter(|h| !h.is_nan()).count()
    }

    /// The cell containing a world position (ignoring Y).
    pub fn cell_at(&self, position: Vec3) -> Option<Cell> {
        let local = (Vec2::new(position.x, position.z) - self.origin) / self.cell_size;
        let cell = (local.x.floor() as i32, local.y.floor() as i32);
        self.in_bounds(cell).then_some(cell)
    }

    fn cell_center_xz(&self, (x, z): Cell) -> Vec2 {
        self.origin + (Vec2::new(x as f32, z as f32) + 0.5) * self.cell_size
    }

    /// Centre of a cell on its floor (Y = 0 when not walkable).
    pub fn cell_center(&self, cell: Cell) -> Vec3 {
        let xz = self.cell_center_xz(cell);
        Vec3::new(xz.x, self.height(cell).unwrap_or(0.0), xz.y)
    }

    /// The walkable cell closest to `position` within `max_distance` metres.
    pub fn nearest_walkable(&self, position: Vec3, max_distance: f32) -> Option<Cell> {
        if !position.is_finite() || max_distance.is_nan() || max_distance < 0.0 {
            return None;
        }
        let local = (Vec2::new(position.x, position.z) - self.origin) / self.cell_size;
        let center = (local.x.floor() as i32, local.y.floor() as i32);
        // Rings past the far edge of the grid hold no cells: never search further than that,
        // whatever `max_distance` is (even infinite).
        let to_edge = [
            center.0.saturating_abs(),
            center.0.saturating_sub(self.width).saturating_abs(),
            center.1.saturating_abs(),
            center.1.saturating_sub(self.depth).saturating_abs(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        .saturating_add(1);
        let reach = ((max_distance / self.cell_size).ceil() as i32).min(to_edge);
        let target = Vec2::new(position.x, position.z);
        let mut best: Option<(f32, Cell)> = None;
        // Rings closer than the grid's nearest edge hold no cells either.
        let first = [
            -center.0,
            center.0.saturating_sub(self.width - 1),
            -center.1,
            center.1.saturating_sub(self.depth - 1),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        .max(0);
        let consider = |best: &mut Option<(f32, Cell)>, cell: Cell| {
            if self.is_walkable(cell) {
                let distance = self.cell_center_xz(cell).distance(target);
                if best.is_none_or(|(d, _)| distance < d) {
                    *best = Some((distance, cell));
                }
            }
        };
        for ring in first..=reach {
            // Only the ring's border, and only the part inside the grid.
            let (x0, x1) = (center.0.saturating_sub(ring), center.0.saturating_add(ring));
            let (z0, z1) = (center.1.saturating_sub(ring), center.1.saturating_add(ring));
            let (xs, xe) = (x0.max(0), x1.min(self.width - 1));
            let (zs, ze) = (z0.max(0), z1.min(self.depth - 1));
            for z in [z0, z1] {
                if (0..self.depth).contains(&z) {
                    (xs..=xe).for_each(|x| consider(&mut best, (x, z)));
                }
                if ring == 0 {
                    break;
                }
            }
            for x in [x0, x1] {
                if (0..self.width).contains(&x) && ring > 0 {
                    (zs.max(z0 + 1)..=ze.min(z1 - 1)).for_each(|z| consider(&mut best, (x, z)));
                }
            }
            // Cells in later rings are at least `ring` cells away.
            if let Some((distance, cell)) = best {
                if distance <= ring as f32 * self.cell_size {
                    return (distance <= max_distance).then_some(cell);
                }
            }
        }
        best.filter(|(d, _)| *d <= max_distance).map(|(_, c)| c)
    }

    fn can_step(&self, from: Cell, to: Cell) -> bool {
        match (self.height(from), self.height(to)) {
            (Some(a), Some(b)) => (a - b).abs() <= self.max_step,
            _ => false,
        }
    }

    /// Shortest path from `from` to `to` as world-space waypoints (ending at `to`'s cell,
    /// or at `to` itself when it is on the grid). Positions off the grid are moved to the
    /// nearest walkable cell within 2 m. `None` if no path exists.
    pub fn find_path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let start = self.nearest_walkable(from, 2.0)?;
        let goal = self.nearest_walkable(to, 2.0)?;
        let cells = self.astar(start, goal)?;
        let mut points: Vec<Vec3> = self
            .smooth(&cells)
            .into_iter()
            .map(|c| self.cell_center(c))
            .collect();
        // End exactly at the target when it lies in the goal cell.
        if self.cell_at(to) == Some(goal) {
            if let Some(last) = points.last_mut() {
                *last = Vec3::new(to.x, last.y, to.z);
            }
        }
        if points.len() > 1 {
            points.remove(0); // the agent is already there
        }
        Some(points)
    }

    fn astar(&self, start: Cell, goal: Cell) -> Option<Vec<Cell>> {
        #[derive(PartialEq)]
        struct Open {
            f: f32,
            cell: Cell,
        }
        impl Eq for Open {}
        impl Ord for Open {
            fn cmp(&self, other: &Self) -> Ordering {
                other.f.total_cmp(&self.f) // min-heap
            }
        }
        impl PartialOrd for Open {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let heuristic = |(x, z): Cell| {
            let (dx, dz) = ((x - goal.0).abs() as f32, (z - goal.1).abs() as f32);
            dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)
        };
        let size = self.heights.len();
        let mut cost = vec![f32::INFINITY; size];
        let mut came_from = vec![u32::MAX; size];
        let mut open = BinaryHeap::new();
        cost[self.index(start)] = 0.0;
        open.push(Open {
            f: heuristic(start),
            cell: start,
        });
        while let Some(Open { f, cell }) = open.pop() {
            if cell == goal {
                let mut path = vec![goal];
                let mut index = self.index(goal);
                while came_from[index] != u32::MAX {
                    index = came_from[index] as usize;
                    path.push(((index as i32) % self.width, (index as i32) / self.width));
                }
                path.reverse();
                return Some(path);
            }
            let g = cost[self.index(cell)];
            if f > g + heuristic(cell) + 1e-4 {
                continue; // stale entry
            }
            for (dx, dz) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let next = (cell.0 + dx, cell.1 + dz);
                if !self.can_step(cell, next) {
                    continue;
                }
                let diagonal = dx != 0 && dz != 0;
                // No cutting corners past walls.
                if diagonal
                    && !(self.can_step(cell, (cell.0 + dx, cell.1))
                        && self.can_step(cell, (cell.0, cell.1 + dz)))
                {
                    continue;
                }
                let step = if diagonal {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                };
                let next_cost = g + step;
                let index = self.index(next);
                if next_cost < cost[index] {
                    cost[index] = next_cost;
                    came_from[index] = self.index(cell) as u32;
                    open.push(Open {
                        f: next_cost + heuristic(next),
                        cell: next,
                    });
                }
            }
        }
        None
    }

    /// Drops intermediate cells while a straight walk stays on walkable cells.
    fn smooth(&self, cells: &[Cell]) -> Vec<Cell> {
        let Some(&first) = cells.first() else {
            return Vec::new();
        };
        let mut out = vec![first];
        let mut anchor = 0;
        while anchor < cells.len() - 1 {
            let mut next = anchor + 1;
            for candidate in (anchor + 2..cells.len()).rev() {
                if self.clear_line(cells[anchor], cells[candidate]) {
                    next = candidate;
                    break;
                }
            }
            out.push(cells[next]);
            anchor = next;
        }
        out
    }

    /// Whether every cell touched by the segment between two cell centres (widened to the
    /// cells beside it, as the agent has width) is walkable and steppable.
    fn clear_line(&self, a: Cell, b: Cell) -> bool {
        let start = Vec2::new(a.0 as f32 + 0.5, a.1 as f32 + 0.5);
        let end = Vec2::new(b.0 as f32 + 0.5, b.1 as f32 + 0.5);
        let steps = ((end - start).length() * 4.0).ceil().max(1.0) as i32;
        let mut previous = a;
        for i in 0..=steps {
            let p = start.lerp(end, i as f32 / steps as f32);
            // Sample slightly off the line on both sides.
            for offset in [
                Vec2::ZERO,
                Vec2::splat(0.3),
                Vec2::splat(-0.3),
                Vec2::new(0.3, -0.3),
                Vec2::new(-0.3, 0.3),
            ] {
                let q = p + offset;
                let cell = (q.x.floor() as i32, q.y.floor() as i32);
                if !self.is_walkable(cell) {
                    return false;
                }
            }
            let cell = (p.x.floor() as i32, p.y.floor() as i32);
            if cell != previous && !self.can_step(previous, cell) {
                return false;
            }
            previous = cell;
        }
        true
    }
}

impl fmt::Debug for NavGrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NavGrid")
            .field("origin", &self.origin)
            .field("cell_size", &self.cell_size)
            .field("size", &(self.width, self.depth))
            .field("walkable", &self.walkable_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10×10 open grid with a wall at x = 5 from z = 0 to z = 7.
    fn walled() -> NavGrid {
        let mut grid = NavGrid::new(Vec2::ZERO, Vec2::splat(10.0), 1.0);
        for z in 0..10 {
            for x in 0..10 {
                let wall = x == 5 && z < 8;
                grid.set_walkable((x, z), (!wall).then_some(0.0));
            }
        }
        grid.max_step = 0.3;
        grid
    }

    /// The nearest walkable cell by checking every cell.
    fn nearest_brute_force(grid: &NavGrid, position: Vec3, max_distance: f32) -> Option<f32> {
        let target = Vec2::new(position.x, position.z);
        (0..grid.depth())
            .flat_map(|z| (0..grid.width()).map(move |x| (x, z)))
            .filter(|&c| grid.is_walkable(c))
            .map(|c| grid.cell_center_xz(c).distance(target))
            .filter(|&d| d <= max_distance)
            .min_by(f32::total_cmp)
    }

    #[test]
    fn nearest_walkable_matches_a_full_search() {
        let mut grid = NavGrid::new(Vec2::new(-3.0, 2.0), Vec2::new(9.0, 9.0), 0.5);
        let mut seed = 12345u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for z in 0..grid.depth() {
            for x in 0..grid.width() {
                grid.set_walkable((x, z), (next() % 17 == 0).then_some(0.0));
            }
        }
        for i in 0..400 {
            let position = Vec3::new(
                (next() % 4000) as f32 / 100.0 - 15.0,
                0.0,
                (next() % 4000) as f32 / 100.0 - 15.0,
            );
            let max_distance = [1.0, 3.0, 40.0, f32::INFINITY][i % 4];
            let found = grid.nearest_walkable(position, max_distance).map(|c| {
                grid.cell_center_xz(c)
                    .distance(Vec2::new(position.x, position.z))
            });
            let expected = nearest_brute_force(&grid, position, max_distance);
            match (found, expected) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-4, "{position} {a} {b}"),
                (a, b) => assert_eq!(a, b, "{position} within {max_distance}"),
            }
        }
    }

    #[test]
    fn far_away_and_bad_queries_return_quickly() {
        let grid = walled();
        let far = Vec3::new(1.0e9, 0.0, -1.0e9);
        let start = std::time::Instant::now();
        assert!(grid.nearest_walkable(far, f32::INFINITY).is_some());
        assert!(grid.nearest_walkable(far, 10.0).is_none());
        assert!(grid.nearest_walkable(Vec3::NAN, 5.0).is_none());
        assert!(grid.nearest_walkable(Vec3::ZERO, f32::NAN).is_none());
        assert!(start.elapsed().as_secs_f32() < 1.0);
    }

    #[test]
    fn huge_grids_get_coarser_cells() {
        let grid = NavGrid::new(Vec2::ZERO, Vec2::splat(2000.0), 0.01);
        assert!((grid.width() as u64 * grid.depth() as u64) <= NavGrid::MAX_CELLS);
        let bad = NavGrid::new(Vec2::NAN, Vec2::splat(f32::INFINITY), f32::NAN);
        assert!(bad.width() >= 1 && bad.depth() >= 1);
    }

    #[test]
    fn straight_paths_are_one_segment() {
        let grid = walled();
        let path = grid
            .find_path(Vec3::new(0.5, 0.0, 9.5), Vec3::new(9.5, 0.0, 9.5))
            .unwrap();
        assert_eq!(path, [Vec3::new(9.5, 0.0, 9.5)]);
    }

    #[test]
    fn paths_go_around_walls() {
        let grid = walled();
        let path = grid
            .find_path(Vec3::new(1.5, 0.0, 1.5), Vec3::new(8.5, 0.0, 1.5))
            .unwrap();
        assert!(path.len() >= 2, "{path:?}");
        // Must pass the end of the wall (z >= 8) and finish at the target.
        assert!(path.iter().any(|p| p.z >= 8.0), "{path:?}");
        assert_eq!(*path.last().unwrap(), Vec3::new(8.5, 0.0, 1.5));
        // Every leg is walkable.
        let mut from = Vec3::new(1.5, 0.0, 1.5);
        for &to in &path {
            for i in 0..=20 {
                let p = from.lerp(to, i as f32 / 20.0);
                assert!(
                    grid.is_walkable(grid.cell_at(p).unwrap()),
                    "{p} crosses the wall"
                );
            }
            from = to;
        }
    }

    #[test]
    fn no_path_through_a_closed_wall_or_big_step() {
        let mut grid = walled();
        for z in 8..10 {
            grid.set_walkable((5, z), None);
        }
        assert!(grid
            .find_path(Vec3::new(1.5, 0.0, 1.5), Vec3::new(8.5, 0.0, 1.5))
            .is_none());
        // Re-open with a ledge too tall to climb.
        grid.set_walkable((5, 9), Some(1.0));
        assert!(grid
            .find_path(Vec3::new(1.5, 0.0, 1.5), Vec3::new(8.5, 0.0, 1.5))
            .is_none());
        grid.set_walkable((5, 9), Some(0.2));
        assert!(grid
            .find_path(Vec3::new(1.5, 0.0, 1.5), Vec3::new(8.5, 0.0, 1.5))
            .is_some());
    }

    #[test]
    fn off_grid_positions_snap_to_the_nearest_cell() {
        let grid = walled();
        // Inside the wall: snaps beside it.
        let cell = grid
            .nearest_walkable(Vec3::new(5.5, 0.0, 3.5), 2.0)
            .unwrap();
        assert!(
            grid.is_walkable(cell) && (cell.0 == 4 || cell.0 == 6),
            "{cell:?}"
        );
        assert!(grid
            .nearest_walkable(Vec3::new(50.0, 0.0, 50.0), 2.0)
            .is_none());
        assert_eq!(grid.cell_at(Vec3::new(-0.1, 0.0, 0.0)), None);
    }
}
