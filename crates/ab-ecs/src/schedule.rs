use crate::system::{IntoSystem, System};
use crate::world::World;
use std::fmt;
use std::time::{Duration, Instant};

/// Execution stages, in the order the engine runs them each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScheduleStage {
    /// Once, before the first frame.
    Startup,
    /// Input polling, event collection.
    PreUpdate,
    /// Fixed timestep, before `physics_process`. May run 0..n times per frame.
    FixedUpdate,
    /// Fixed timestep, after `physics_process`: physics step and sync, so what scripts set
    /// this step (velocities, impulses, moved nodes) is simulated right away.
    PostFixedUpdate,
    /// Variable timestep: `process`.
    Update,
    /// Transform propagation, audio listener updates.
    PostUpdate,
    /// Render extraction and submission.
    Render,
    /// Deferred frees and end-of-frame bookkeeping.
    Cleanup,
}

impl ScheduleStage {
    pub const ALL: [ScheduleStage; 8] = [
        ScheduleStage::Startup,
        ScheduleStage::PreUpdate,
        ScheduleStage::FixedUpdate,
        ScheduleStage::PostFixedUpdate,
        ScheduleStage::Update,
        ScheduleStage::PostUpdate,
        ScheduleStage::Render,
        ScheduleStage::Cleanup,
    ];

    const fn slot(self) -> usize {
        self as usize
    }
}

/// Systems grouped by [`ScheduleStage`], run in insertion order within a stage.
/// Queued [`Commands`](crate::Commands) are flushed after every system.
#[derive(Default)]
pub struct Schedule {
    stages: [Vec<Box<dyn System>>; ScheduleStage::ALL.len()],
    /// Time spent in each system since the last [`Schedule::reset_timings`].
    timings: [Vec<Duration>; ScheduleStage::ALL.len()],
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_system(&mut self, stage: ScheduleStage, system: impl IntoSystem) -> &mut Self {
        self.stages[stage.slot()].push(system.into_system());
        self.timings[stage.slot()].push(Duration::ZERO);
        self
    }

    pub fn run_stage(&mut self, stage: ScheduleStage, world: &mut World) {
        let timings = &mut self.timings[stage.slot()];
        for (system, time) in self.stages[stage.slot()].iter_mut().zip(timings) {
            let start = Instant::now();
            system.run(world);
            world.flush();
            *time += start.elapsed();
        }
    }

    /// Clears the per-system timings (the engine does it at the start of every frame).
    pub fn reset_timings(&mut self) {
        for stage in &mut self.timings {
            stage.fill(Duration::ZERO);
        }
    }

    /// Time each system of `stage` took since the last reset (all runs of a fixed stage
    /// added up), in insertion order.
    pub fn system_timings(&self, stage: ScheduleStage) -> impl Iterator<Item = (&str, Duration)> {
        self.stages[stage.slot()]
            .iter()
            .zip(&self.timings[stage.slot()])
            .map(|(system, time)| (system.name(), *time))
    }

    pub fn system_count(&self, stage: ScheduleStage) -> usize {
        self.stages[stage.slot()].len()
    }

    pub fn system_names(&self, stage: ScheduleStage) -> impl Iterator<Item = &str> {
        self.stages[stage.slot()].iter().map(|s| s.name())
    }
}

impl fmt::Debug for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for stage in ScheduleStage::ALL {
            map.entry(&stage, &self.system_names(stage).collect::<Vec<_>>());
        }
        map.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::named;

    #[derive(Default)]
    struct Log(Vec<&'static str>);

    #[test]
    fn runs_systems_in_stage_and_insertion_order() {
        let mut world = World::new();
        world.insert_resource(Log::default());
        let mut schedule = Schedule::new();
        schedule
            .add_system(ScheduleStage::Update, |w: &mut World| {
                w.resource_mut::<Log>().unwrap().0.push("update-1")
            })
            .add_system(ScheduleStage::Update, |w: &mut World| {
                w.resource_mut::<Log>().unwrap().0.push("update-2")
            })
            .add_system(
                ScheduleStage::PostUpdate,
                named("post", |w: &mut World| {
                    w.resource_mut::<Log>().unwrap().0.push("post")
                }),
            );

        schedule.run_stage(ScheduleStage::Update, &mut world);
        schedule.run_stage(ScheduleStage::PostUpdate, &mut world);
        schedule.run_stage(ScheduleStage::Render, &mut world);

        assert_eq!(
            world.resource::<Log>().unwrap().0,
            ["update-1", "update-2", "post"]
        );
        assert_eq!(
            schedule.system_names(ScheduleStage::PostUpdate).next(),
            Some("post")
        );
    }

    #[test]
    fn systems_are_timed() {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule.add_system(
            ScheduleStage::Update,
            named("sleepy", |_: &mut World| {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }),
        );
        schedule.run_stage(ScheduleStage::Update, &mut world);
        schedule.run_stage(ScheduleStage::Update, &mut world);
        let (name, time) = schedule
            .system_timings(ScheduleStage::Update)
            .next()
            .unwrap();
        assert_eq!(name, "sleepy");
        assert!(
            time >= std::time::Duration::from_millis(4),
            "two runs add up: {time:?}"
        );
        schedule.reset_timings();
        assert_eq!(
            schedule
                .system_timings(ScheduleStage::Update)
                .next()
                .unwrap()
                .1,
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn commands_are_flushed_between_systems() {
        struct Marker;
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule
            .add_system(ScheduleStage::Update, |w: &mut World| {
                w.commands().spawn((Marker,))
            })
            .add_system(ScheduleStage::Update, |w: &mut World| {
                assert_eq!(w.storage::<Marker>().map_or(0, |s| s.len()), 1);
            });
        schedule.run_stage(ScheduleStage::Update, &mut world);
    }
}
