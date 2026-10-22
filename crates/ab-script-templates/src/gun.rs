//! Gun: hitscan shooting from the camera's centre, automatic or not, with a magazine,
//! reloading, recoil, spread that grows while firing and moving, a muzzle flash, impacts
//! (blood on things with `Health`, sparks elsewhere, a push for physics bodies) and an ammo
//! counter on screen.
//!
//! Put it on the player (with a camera below it, like the `FirstPersonController` makes, or
//! any current camera). Actions: `fire`, `reload`, optionally `aim` (steadier while held).
//!
//! Signals it emits on its node: `fired`, `reloaded`, `hit` (damage), `ammo_changed`
//! (rounds in the magazine). The `Crosshair` script listens to `fired` and `hit`.

use ab_script::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Gun {
    pub damage: f32,
    /// Shots per second.
    pub fire_rate: f32,
    /// Keep firing while `fire` is held.
    pub automatic: bool,
    pub magazine: u32,
    /// Rounds carried besides the magazine.
    pub reserve: u32,
    pub reload_time: f32,
    pub range: f32,
    /// Spread in degrees: still and aiming, growth per shot, the most it reaches.
    pub spread: f32,
    pub spread_per_shot: f32,
    pub max_spread: f32,
    /// Camera kick per shot, in degrees, and how fast it settles (per second).
    pub recoil: f32,
    pub recoil_recovery: f32,
    /// Push given to physics bodies hit, in N·s.
    pub impact_force: f32,
    /// Sound played per shot (a path under assets/, empty for none).
    pub fire_sound: String,
    #[serde(skip)]
    ammo: Option<u32>,
    #[serde(skip)]
    cooldown: f32,
    #[serde(skip)]
    reloading: f32,
    #[serde(skip)]
    heat: f32,
    #[serde(skip)]
    kick: f32,
    #[serde(skip)]
    camera: Option<Node>,
    #[serde(skip)]
    label: Option<Node>,
    #[serde(skip)]
    rng: Option<Rng>,
}

impl Default for Gun {
    fn default() -> Self {
        Self {
            damage: 25.0,
            fire_rate: 9.0,
            automatic: true,
            magazine: 30,
            reserve: 90,
            reload_time: 1.6,
            range: 120.0,
            spread: 0.3,
            spread_per_shot: 0.5,
            max_spread: 4.0,
            recoil: 1.2,
            recoil_recovery: 8.0,
            impact_force: 4.0,
            fire_sound: String::new(),
            ammo: None,
            cooldown: 0.0,
            reloading: 0.0,
            heat: 0.0,
            kick: 0.0,
            camera: None,
            label: None,
            rng: None,
        }
    }
}

pub const FIRED: &str = "fired";
pub const RELOADED: &str = "reloaded";
pub const HIT: &str = "hit";
pub const AMMO_CHANGED: &str = "ammo_changed";

impl Gun {
    fn ammo(&self) -> u32 {
        self.ammo.unwrap_or(self.magazine)
    }

    fn start_reload(&mut self) {
        if self.reloading <= 0.0 && self.ammo() < self.magazine && self.reserve > 0 {
            self.reloading = self.reload_time;
        }
    }

    fn fire(&mut self, tree: &mut Tree<'_>, node: Node) {
        let Some(view) = self.camera.and_then(|c| tree.global_transform(c)) else {
            return;
        };
        self.ammo = Some(self.ammo() - 1);
        self.cooldown = 1.0 / self.fire_rate.max(0.1);
        // Spread: a random direction in a cone that widens as the barrel heats up.
        let aiming = tree.input().is_pressed("aim");
        let moving = tree
            .character(node)
            .velocity()
            .map_or(0.0, |v| Vec2::new(v.x, v.z).length());
        let degrees = (self.spread + self.heat + moving * 0.3) * if aiming { 0.4 } else { 1.0 };
        let rng = self.rng.get_or_insert_with(Rng::from_entropy);
        let angle = rng.range_f32(0.0, std::f32::consts::TAU);
        let off = rng.next_f32().sqrt() * degrees.to_radians();
        let direction = (view.forward()
            + (view.right() * angle.cos() + view.up() * angle.sin()) * off.tan())
        .normalize();
        self.heat = (self.heat + self.spread_per_shot).min(self.max_spread);
        self.kick += self.recoil.to_radians();

        let options = QueryOptions::new().excluding(node);
        let origin = view.translation;
        if let Some(hit) = tree
            .physics()
            .cast_ray(origin, direction, self.range, &options)
        {
            let root = tree.root();
            match damageable(tree, hit.node) {
                Some(target) => {
                    let _ = tree.deal_damage(Some(node), target, self.damage);
                    let _ = tree.emit_with(node, HIT, self.damage);
                    let blood = Blood {
                        amount: 0.6,
                        splats: 1,
                        ..Blood::along(direction + hit.normal)
                    };
                    let _ = tree.spawn_blood(root, hit.position, blood);
                }
                None => {
                    let sparks = ParticleEffect::preset("sparks")
                        .with_amount(10)
                        .with_direction(hit.normal, 50.0)
                        .with_speed(1.5, 4.0);
                    let _ = tree.spawn_effect(root, hit.position, &sparks);
                }
            }
            let _ = tree
                .body(hit.node)
                .apply_impulse_at(direction * self.impact_force, hit.position);
        }
        // Muzzle flash a little in front of the camera, low and to the right.
        let muzzle = origin + view.forward() * 0.6 + view.right() * 0.18 - view.up() * 0.15;
        let flash = ParticleEffect::preset("sparks")
            .with_amount(6)
            .with_lifetime(0.06)
            .with_size(0.08, 0.02)
            .with_direction(view.forward(), 20.0);
        let root = tree.root();
        let _ = tree.spawn_effect(root, muzzle, &flash);
        if !self.fire_sound.is_empty() {
            let _ = tree.play_sound(&self.fire_sound, SoundOptions::default());
        }
        let _ = tree.emit(node, FIRED);
        let _ = tree.emit_with(node, AMMO_CHANGED, self.ammo());
    }

    fn update_label(&self, tree: &mut Tree<'_>) {
        let Some(label) = self.label else {
            return;
        };
        let text = if self.reloading > 0.0 {
            "Reloading…".to_owned()
        } else {
            format!("{} / {}", self.ammo(), self.reserve)
        };
        let _ = tree.label(label).set_text(&text);
    }
}

/// `node` or the nearest ancestor in the `damageable` group.
fn damageable(tree: &Tree<'_>, node: Node) -> Option<Node> {
    let mut current = Some(node);
    while let Some(n) = current {
        if tree.is_in_group(n, "damageable") {
            return Some(n);
        }
        current = tree.parent(n);
    }
    None
}

/// The camera below `node`, else the first current camera in the scene.
fn find_camera(tree: &mut Tree<'_>, node: Node) -> Option<Node> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if tree.class_name(n).as_deref() == Some("Camera3D") {
            return Some(n);
        }
        stack.extend(tree.children(n));
    }
    let mut stack = vec![tree.root()];
    while let Some(n) = stack.pop() {
        if tree.class_name(n).as_deref() == Some("Camera3D") && tree.camera(n).is_current() {
            return Some(n);
        }
        stack.extend(tree.children(n));
    }
    None
}

impl Script for Gun {
    serde_state!();

    fn ready(&mut self, tree: &mut Tree<'_>, _node: Node) {
        // An ammo counter at the bottom right.
        let layer = tree.create_canvas_layer("GunHud", 10).ok();
        let label = tree.create_label("Ammo", "").ok();
        if let (Some(layer), Some(label)) = (layer, label) {
            let root = tree.root();
            let _ = tree.add_child(root, layer);
            let _ = tree.add_child(layer, label);
            let _ = tree.control(label).set_preset(
                LayoutPreset::BottomRight,
                Vec2::new(220.0, 48.0),
                24.0,
            );
            let _ = tree.label(label).set_align(HAlign::Right, VAlign::Center);
            self.label = Some(label);
        }
        self.update_label(tree);
    }

    fn process(&mut self, tree: &mut Tree<'_>, node: Node, delta: f32) {
        if self.camera.is_none_or(|c| !tree.is_valid(c)) {
            self.camera = find_camera(tree, node);
        }
        self.cooldown -= delta;
        self.heat = (self.heat - self.max_spread * delta * 1.5).max(0.0);
        // Recoil: kick the camera up, then let it settle.
        let settled = self.kick * (-self.recoil_recovery * delta).exp();
        if let Some(camera) = self.camera {
            let _ = tree.set_rotation(camera, Quat::from_rotation_x(settled));
        }
        self.kick = settled;

        if self.reloading > 0.0 {
            self.reloading -= delta;
            if self.reloading <= 0.0 {
                let taken = (self.magazine - self.ammo()).min(self.reserve);
                self.reserve -= taken;
                self.ammo = Some(self.ammo() + taken);
                let _ = tree.emit(node, RELOADED);
                let _ = tree.emit_with(node, AMMO_CHANGED, self.ammo());
            }
        } else if tree.input().is_just_pressed("reload") {
            self.start_reload();
        } else {
            let trigger = if self.automatic {
                tree.input().is_pressed("fire")
            } else {
                tree.input().is_just_pressed("fire")
            };
            if trigger && self.cooldown <= 0.0 {
                if self.ammo() > 0 {
                    self.fire(tree, node);
                } else {
                    // Empty: reload by itself.
                    self.start_reload();
                }
            }
        }
        self.update_label(tree);
    }
}
