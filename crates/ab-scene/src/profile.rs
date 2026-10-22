//! Time spent in scripts, per script type.

use std::time::Duration;

/// Accumulated time of one script type since the last [`ScriptProfile::take`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptTiming {
    /// Short type name (`Player` for `my_game::player::Player`).
    pub name: &'static str,
    pub total: Duration,
    /// `process` / `physics_process` calls.
    pub calls: u32,
}

/// ECS resource: how long each script type's `process` and `physics_process` took. The
/// engine drains it every frame into its profiler (`script.<Name>` entries).
#[derive(Debug, Clone)]
pub struct ScriptProfile {
    /// Measure (two clock reads per script call).
    pub enabled: bool,
    timings: Vec<ScriptTiming>,
}

impl Default for ScriptProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            timings: Vec::new(),
        }
    }
}

impl ScriptProfile {
    pub(crate) fn add(&mut self, type_name: &'static str, duration: Duration) {
        let name = short_name(type_name);
        match self.timings.iter_mut().find(|t| t.name == name) {
            Some(timing) => {
                timing.total += duration;
                timing.calls += 1;
            }
            None => self.timings.push(ScriptTiming {
                name,
                total: duration,
                calls: 1,
            }),
        }
    }

    /// The timings collected so far, leaving the profile empty.
    pub fn take(&mut self) -> Vec<ScriptTiming> {
        std::mem::take(&mut self.timings)
    }
}

/// `a::b::Player` → `Player`, `a::Wrapper<b::C>` → `Wrapper<b::C>`.
fn short_name(type_name: &'static str) -> &'static str {
    let path_end = type_name.find('<').unwrap_or(type_name.len());
    match type_name[..path_end].rfind("::") {
        Some(i) => &type_name[i + 2..],
        None => type_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sums() {
        assert_eq!(short_name("game::player::Player"), "Player");
        assert_eq!(short_name("Player"), "Player");
        assert_eq!(short_name("a::Wrap<b::C>"), "Wrap<b::C>");
        let mut profile = ScriptProfile::default();
        profile.add("x::A", Duration::from_millis(2));
        profile.add("y::A", Duration::from_millis(3));
        profile.add("x::B", Duration::from_millis(1));
        let timings = profile.take();
        assert_eq!(timings.len(), 2);
        assert_eq!(timings[0].total, Duration::from_millis(5));
        assert_eq!(timings[0].calls, 2);
        assert!(profile.take().is_empty());
    }
}
