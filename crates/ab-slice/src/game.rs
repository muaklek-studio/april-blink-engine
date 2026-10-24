use crate::ghost::Ghost;
use crate::hud::{self, Hud};
use crate::player::Player;
use crate::{layers, LEVEL, PLACED, RELICS, RELIC_PLACED};
use ab_script::prelude::*;
use ab_script::signals::BODY_ENTERED;

/// Seconds from midnight to dawn.
const NIGHT: f32 = 180.0;
/// Each relic on the altar brings dawn this much closer.
const DAWN_PER_RELIC: f32 = 30.0;
/// ...and makes the ghost this much faster (m/s).
const GHOST_SPEEDUP: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Loading,
    Playing,
    Won,
    Lost,
}

/// Runs a round: loads the level, counts down to dawn, reacts to relics and the ghost,
/// shows the HUD, and restarts on demand.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Game {
    phase: Phase,
    /// Seconds until dawn.
    dawn: f32,
    placed: u32,
    total: u32,
    level: Option<Node>,
    player: Option<Node>,
    ghost: Option<Node>,
    hud: Hud,
    /// Physics steps to wait before baking navigation (the new level must be stepped).
    bake_in: u32,
    restart_pending: bool,
    elapsed: f32,
}

impl Game {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn dawn(&self) -> f32 {
        self.dawn
    }

    pub fn placed(&self) -> u32 {
        self.placed
    }

    pub fn level(&self) -> Option<Node> {
        self.level
    }

    pub fn player(&self) -> Option<Node> {
        self.player
    }

    pub fn ghost(&self) -> Option<Node> {
        self.ghost
    }

    /// Ends the round early (tests, debugging).
    pub fn set_dawn(&mut self, seconds: f32) {
        self.dawn = seconds;
    }

    fn start(&mut self, tree: &mut Tree<'_>, me: Node) {
        let level = match tree.load_scene(LEVEL) {
            Ok(level) => level,
            Err(err) => {
                error!("cannot load the level: {err}");
                hud::set_text(tree, self.hud.dawn, &err.to_string());
                return;
            }
        };
        if tree.add_child(me, level).is_err() {
            return;
        }
        self.level = Some(level);
        self.player = tree.get_node(level, "Player");
        self.ghost = tree.get_node(level, "Ghost");
        if let Some(player) = self.player {
            let _ = tree.connect(player, RELIC_PLACED, me);
        }
        if let Some(reach) = tree.get_node(level, "Ghost/Reach") {
            let _ = tree.connect(reach, BODY_ENTERED, me);
        }
        self.total = tree.nodes_in_group(RELICS).len() as u32;
        self.placed = 0;
        self.dawn = NIGHT;
        self.phase = Phase::Playing;
        self.bake_in = 2;
        self.elapsed = 0.0;
        tree.engine().set_mouse_captured(true);
        if let Some(panel) = self.hud.end_panel {
            let _ = tree.set_visible(panel, false);
        }
        if let Some(hint) = self.hud.hint {
            let mut control = tree.control(hint);
            let _ = control.stop_animations();
            let _ = control.set_alpha(1.0);
            let _ = control.tween_alpha(1.0, 0.0, 1.5, 10.0, Easing::EaseIn);
        }
        let hud = self.hud;
        hud::toast(
            tree,
            &hud,
            "นำเครื่องรางไปวางบนแท่นบูชา แล้วเอาตัวรอดจนรุ่งเช้า · Bring the relics to the altar and survive until dawn",
        );
        info!("round started with {} relics", self.total);
    }

    fn finish(&mut self, tree: &mut Tree<'_>, won: bool) {
        if self.phase != Phase::Playing {
            return;
        }
        self.phase = if won { Phase::Won } else { Phase::Lost };
        for node in [self.player, self.ghost].into_iter().flatten() {
            let _ = tree.set_process(node, false);
            let _ = tree.set_physics_process(node, false);
        }
        tree.engine().set_mouse_captured(false);
        let (title, text) = if won {
            (
                "รุ่งเช้าแล้ว · Dawn",
                format!(
                    "คุณรอดมาได้ และวางเครื่องราง {}/{} ชิ้น · You survived with {}/{} relics placed",
                    self.placed, self.total, self.placed, self.total
                ),
            )
        } else {
            (
                "ผีจับคุณได้ · Caught",
                format!(
                    "เหลืออีก {} ก่อนรุ่งสาง · {} left until dawn",
                    clock(self.dawn),
                    clock(self.dawn)
                ),
            )
        };
        hud::set_text(tree, self.hud.end_title, title);
        hud::set_text(
            tree,
            self.hud.end_text,
            &format!("{text}\nR  เริ่มใหม่ · Restart    Esc  ออก · Quit"),
        );
        if let Some(panel) = self.hud.end_panel {
            let _ = tree.set_visible(panel, true);
            let mut control = tree.control(panel);
            let _ = control.stop_animations();
            let _ = control.fade(0.0, 1.0, 0.6);
            let _ = control.slide_in(Vec2::new(0.0, 40.0), 0.6);
        }
        hud::set_text(tree, self.hud.prompt, "");
        info!(
            "round over: {} with {} relics placed and {:.1} s to dawn",
            if won { "won" } else { "lost" },
            self.placed,
            self.dawn
        );
    }

    fn update_hud(&self, tree: &mut Tree<'_>) {
        let warning = if self.dawn < 30.0 && self.phase == Phase::Playing {
            " ✦"
        } else {
            ""
        };
        hud::set_text(
            tree,
            self.hud.dawn,
            &format!(
                "รุ่งสางใน {0} · Dawn in {0}{warning}",
                clock(self.dawn.max(0.0))
            ),
        );
        hud::set_text(
            tree,
            self.hud.relics,
            &format!(
                "เครื่องรางบนแท่น {0}/{1} · Relics {0}/{1}",
                self.placed, self.total
            ),
        );
        let (prompt, stamina) = self
            .player
            .and_then(|p| tree.script::<Player>(p))
            .map(|p| (p.prompt().to_owned(), p.stamina))
            .unwrap_or_default();
        if self.phase == Phase::Playing {
            hud::set_text(tree, self.hud.prompt, &prompt);
        }
        if let Some(bar) = self.hud.stamina {
            let _ = tree.progress_bar(bar).set_value(stamina);
        }
    }
}

/// `m:ss`.
fn clock(seconds: f32) -> String {
    let total = seconds.ceil().max(0.0) as u32;
    format!("{}:{:02}", total / 60, total % 60)
}

impl Script for Game {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, node: Node) {
        match hud::build(tree, node) {
            Ok(hud) => self.hud = hud,
            Err(err) => error!("cannot build the HUD: {err}"),
        }
        self.start(tree, node);
    }

    fn physics_process(&mut self, tree: &mut Tree<'_>, _node: Node, _delta: f32) {
        if self.bake_in == 0 {
            return;
        }
        self.bake_in -= 1;
        if self.bake_in > 0 {
            return;
        }
        let settings = NavBakeSettings {
            min: Vec3::new(-12.0, -1.0, -8.0),
            max: Vec3::new(12.0, 4.0, 8.0),
            cell_size: 0.25,
            agent_radius: 0.4,
            agent_height: 1.8,
            collision_mask: layers::LEVEL,
            ..Default::default()
        };
        match tree.bake_navigation(&settings) {
            Ok(walkable) => info!("navigation baked: {walkable} walkable cells"),
            Err(err) => error!("cannot bake navigation: {err}"),
        }
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.restart_pending {
            // The old level was freed at the end of last frame.
            self.restart_pending = false;
            self.start(tree, node);
            return;
        }
        let input = tree.input();
        let (restart, quit, interact) = (
            input.is_just_pressed("restart"),
            input.is_just_pressed("quit"),
            input.is_just_pressed("interact"),
        );
        let captured = tree.engine().is_mouse_captured();
        if quit {
            if captured {
                tree.engine().set_mouse_captured(false);
            } else {
                tree.engine().quit();
            }
        } else if interact && self.phase == Phase::Playing && !captured {
            tree.engine().set_mouse_captured(true);
        }

        match self.phase {
            Phase::Playing => {
                self.elapsed += delta;
                self.dawn -= delta;
                if self.dawn <= 0.0 {
                    self.finish(tree, true);
                }
            }
            Phase::Won | Phase::Lost if restart => {
                if let Some(level) = self.level.take() {
                    let _ = tree.queue_free(level);
                }
                tree.clear_navigation();
                self.phase = Phase::Loading;
                self.restart_pending = true;
            }
            _ => {}
        }
        self.update_hud(tree);
    }

    fn on_signal(&mut self, tree: &mut Tree<'_>, _node: Node, signal: &Signal<'_>) {
        if self.phase != Phase::Playing {
            return;
        }
        if signal.is(RELIC_PLACED) {
            // A flourish of magic where the relic landed.
            if let (Some(relic), Some(level)) = (signal.arg::<Node>(), self.level) {
                if let Some(at) = tree.global_position(relic) {
                    let magic = ParticleEffect::preset("magic").with_amount(60);
                    let _ = tree.spawn_effect(level, at, &magic);
                }
            }
            tree.shake_cameras(0.25);
            self.placed = tree.nodes_in_group(PLACED).len().min(self.total as usize) as u32;
            self.dawn = (self.dawn - DAWN_PER_RELIC).max(0.5);
            if let Some(ghost) = self.ghost.and_then(|g| tree.script_mut::<Ghost>(g)) {
                ghost.speed += GHOST_SPEEDUP;
            }
            let hud = self.hud;
            hud::toast(
                tree,
                &hud,
                "รุ่งสางใกล้เข้ามา แต่ผีโกรธขึ้น · Dawn draws closer, the ghost grows restless",
            );
        } else if signal.is(BODY_ENTERED) && signal.arg::<Node>() == self.player {
            tree.shake_cameras(0.9);
            if let (Some(level), Some(at)) = (
                self.level,
                self.player.and_then(|p| tree.global_position(p)),
            ) {
                let burst = ParticleEffect::preset("wisps")
                    .with_amount(60)
                    .with_speed(1.0, 2.5);
                let _ = tree.spawn_effect(level, at + Vec3::Y * 0.6, &burst);
            }
            self.finish(tree, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_rounds_up_to_whole_seconds() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(0.2), "0:01");
        assert_eq!(clock(59.5), "1:00");
        assert_eq!(clock(61.0), "1:01");
        assert_eq!(clock(NIGHT), "3:00");
        assert_eq!(clock(600.0), "10:00");
    }

    #[test]
    fn clock_never_goes_negative() {
        assert_eq!(clock(-0.4), "0:00");
        assert_eq!(clock(-75.0), "0:00");
    }

    #[test]
    fn a_new_game_is_loading() {
        let game = Game::default();
        assert_eq!(game.phase(), Phase::Loading);
        assert_eq!(game.placed(), 0);
        assert_eq!(game.level(), None);
    }

    #[test]
    fn hot_reload_keeps_the_round() {
        let mut game = Game {
            phase: Phase::Playing,
            placed: 2,
            total: 4,
            ..Game::default()
        };
        game.set_dawn(97.5);
        let state = game.save_state().expect("game state");
        let mut reloaded = Game::default();
        reloaded.load_state(&state);
        assert_eq!(reloaded.phase(), Phase::Playing);
        assert_eq!(reloaded.dawn(), 97.5);
        assert_eq!(reloaded.placed(), 2);
        assert_eq!(reloaded.total, 4);
    }
}
