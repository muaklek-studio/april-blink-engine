//! The Audio Mixer: a channel strip per bus (meter, fader, mute, solo, insert effects and
//! sends), the selected effect's editor, and snapshots. Edits apply live and are saved to
//! the project's `audio.toml`.
//!
//! While a snapshot is being edited, the strips show the mix with the snapshot applied and
//! every change goes into the snapshot instead of the bus.

use crate::audio_widgets::{self as widgets, MeterState, Spectrum};
use crate::i18n::{t, tf};
use crate::state::Editor;
use crate::theme;
use ab_engine::audio::effect::{EffectDesc, EffectKind, EFFECT_KINDS};
use ab_engine::audio::mixer::{BusDesc, BusOverride, ReturnDesc, SendDesc, SnapshotDesc};
use ab_engine::audio::{Audio, MixerSettings, MASTER};
use eframe::egui::{self, RichText, Vec2};
use egui_phosphor::regular as icon;
use std::collections::HashMap;
use std::time::Instant;

const STRIP_WIDTH: f32 = 132.0;
const FADER_HEIGHT: f32 = 210.0;
/// Height of a strip's inserts and sends (scrolls past it).
const ROUTING_HEIGHT: f32 = 150.0;

/// What a strip shows and edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Master,
    Bus,
    Return,
}

/// A bus's settings as a strip edits them (from the bus, or with the edited snapshot).
#[derive(Debug, Clone, PartialEq)]
struct StripValues {
    volume_db: f32,
    mute: bool,
    effects: Vec<EffectDesc>,
    sends: Vec<SendDesc>,
}

#[derive(Default)]
pub struct AudioMixerPanel {
    meters: HashMap<String, MeterState>,
    /// The bus and effect whose editor is open.
    selected: Option<(String, usize)>,
    eq_band: Option<usize>,
    spectrum: Spectrum,
    started: Option<Instant>,
    last: Option<Instant>,
    /// The bus being renamed, with the new name typed so far.
    rename: Option<(String, String)>,
    /// The snapshot being edited (and heard).
    editing: Option<String>,
}

impl AudioMixerPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let now = Instant::now();
        let started = *self.started.get_or_insert(now);
        let delta = self.last.map_or(0.0, |l| (now - l).as_secs_f32());
        self.last = Some(now);
        let clock = (now - started).as_secs_f64();
        // Meters move: keep drawing while the mixer is open.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));

        if self
            .editing
            .as_ref()
            .is_some_and(|s| !editor.mixer.snapshots.iter().any(|d| &d.name == s))
        {
            self.editing = None;
        }
        let mut changed = self.toolbar(ui, editor);
        ui.separator();

        let levels: HashMap<String, _> = {
            let audio = editor.tree().world().resource::<Audio>();
            editor
                .mixer
                .bus_names()
                .filter_map(|name| {
                    let levels = audio.as_ref()?.bus_levels(name)?;
                    Some((name.to_owned(), levels))
                })
                .collect()
        };
        for (name, levels) in &levels {
            let reading = levels.read();
            self.meters
                .entry(name.clone())
                .or_default()
                .update(&reading, clock, delta);
        }

        let strips = order(&editor.mixer);
        if self.selected.is_none() {
            // Something to look at below the strips from the start.
            self.selected = strips.iter().find_map(|(name, kind, _)| {
                let values = self.values(&editor.mixer, name, *kind);
                (!values.effects.is_empty()).then(|| (name.clone(), 0))
            });
        }
        let strip_height = (ui.available_height() * 0.62).max(FADER_HEIGHT + 200.0);
        egui::ScrollArea::horizontal()
            .id_salt("mixer_strips")
            .max_height(strip_height)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for (name, kind, depth) in &strips {
                        changed |= self.strip(ui, editor, name, *kind, *depth, clock);
                    }
                });
            });
        ui.separator();
        changed |= self.effect_editor(ui, editor, &levels, delta);

        if changed {
            editor.mixer_modified = true;
            editor.apply_mixer();
            if let Some(name) = &self.editing {
                // Hear the snapshot being edited, whatever the game asked for.
                if let Some(mut audio) = editor.tree().world().resource_mut::<Audio>() {
                    audio.set_snapshot(name, true);
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor) -> bool {
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            let save = egui::Button::new(format!("{}  {}", icon::FLOPPY_DISK, t("audio.save")));
            if ui
                .add_enabled(editor.mixer_modified, save)
                .on_hover_text(t("audio.save_hint"))
                .clicked()
            {
                match editor.save_mixer() {
                    Ok(()) => editor.info(t("audio.mixer_saved")),
                    Err(err) => editor.fail(format!("{err:#}")),
                }
            }
            if ui
                .add_enabled(
                    editor.mixer_modified,
                    egui::Button::new(format!(
                        "{}  {}",
                        icon::ARROW_COUNTER_CLOCKWISE,
                        t("audio.revert")
                    )),
                )
                .clicked()
            {
                editor.load_mixer();
            }
            ui.separator();
            if ui
                .button(format!("{}  {}", icon::PLUS, t("audio.add_bus")))
                .clicked()
            {
                let name = unique_name(&editor.mixer, "Bus");
                editor.mixer.buses.push(BusDesc::new(&name));
                changed = true;
            }
            if ui
                .button(format!("{}  {}", icon::PLUS, t("audio.add_return")))
                .on_hover_text(t("audio.add_return_hint"))
                .clicked()
            {
                let name = unique_name(&editor.mixer, "Return");
                editor.mixer.returns.push(ReturnDesc {
                    name,
                    effects: vec![EffectDesc::new(
                        EffectKind::from_name("reverb").expect("a built-in effect"),
                    )],
                    ..ReturnDesc::default()
                });
                changed = true;
            }
            ui.separator();
            ui.label(RichText::new(t("audio.max_voices")).color(theme::TEXT_DIM));
            changed |= ui
                .add(egui::DragValue::new(&mut editor.mixer.max_voices).range(1..=1024))
                .on_hover_text(t("audio.max_voices_hint"))
                .changed();
            if let Some(audio) = editor.tree().world().resource::<Audio>() {
                let stats = audio.voice_stats();
                ui.label(
                    RichText::new(tf(
                        "audio.voice_stats",
                        &[
                            ("real", &stats.real),
                            ("virtual", &stats.virtual_voices),
                            ("stolen", &stats.stolen),
                            ("refused", &stats.refused),
                        ],
                    ))
                    .color(theme::TEXT_DIM)
                    .small(),
                );
            }
            ui.separator();
            changed |= self.snapshot_bar(ui, editor);
        });
        let problems = editor.mixer.problems();
        for problem in problems {
            ui.label(RichText::new(format!("{}  {problem}", icon::WARNING)).color(theme::WARN));
        }
        changed
    }

    fn snapshot_bar(&mut self, ui: &mut egui::Ui, editor: &mut Editor) -> bool {
        let mut changed = false;
        ui.label(
            RichText::new(format!("{}  {}", icon::CAMERA, t("audio.snapshot")))
                .color(theme::TEXT_DIM),
        );
        let current = self.editing.clone();
        let label = current.clone().unwrap_or_else(|| t("audio.no_snapshot"));
        let mut pick: Option<Option<String>> = None;
        egui::ComboBox::from_id_salt("mixer_snapshot")
            .selected_text(label)
            .width(140.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), t("audio.no_snapshot"))
                    .clicked()
                {
                    pick = Some(None);
                }
                for s in &editor.mixer.snapshots {
                    if ui
                        .selectable_label(current.as_deref() == Some(s.name.as_str()), &s.name)
                        .clicked()
                    {
                        pick = Some(Some(s.name.clone()));
                    }
                }
            });
        if let Some(pick) = pick {
            self.set_editing(editor, pick);
        }
        if ui
            .button(icon::PLUS)
            .on_hover_text(t("audio.new_snapshot"))
            .clicked()
        {
            let mut n = 1;
            let name = loop {
                let name = format!("Snapshot {n}");
                if !editor.mixer.snapshots.iter().any(|s| s.name == name) {
                    break name;
                }
                n += 1;
            };
            editor.mixer.snapshots.push(SnapshotDesc {
                name: name.clone(),
                ..SnapshotDesc::default()
            });
            self.set_editing(editor, Some(name));
            changed = true;
        }
        if let Some(name) = self.editing.clone() {
            if let Some(snapshot) = editor.mixer.snapshots.iter_mut().find(|s| s.name == name) {
                ui.label(RichText::new(t("audio.fade_in")).color(theme::TEXT_DIM));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut snapshot.fade_in)
                            .range(0.0..=30.0)
                            .speed(0.01)
                            .suffix(" s"),
                    )
                    .changed();
                ui.label(RichText::new(t("audio.fade_out")).color(theme::TEXT_DIM));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut snapshot.fade_out)
                            .range(0.0..=30.0)
                            .speed(0.01)
                            .suffix(" s"),
                    )
                    .changed();
                let mut new_name = snapshot.name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut new_name).desired_width(110.0))
                    .on_hover_text(t("audio.snapshot_name_hint"))
                    .changed()
                    && !new_name.trim().is_empty()
                {
                    snapshot.name = new_name.clone();
                    self.editing = Some(new_name);
                    changed = true;
                }
            }
            if ui
                .button(icon::TRASH)
                .on_hover_text(t("audio.delete_snapshot"))
                .clicked()
            {
                editor.mixer.snapshots.retain(|s| s.name != name);
                self.set_editing(editor, None);
                changed = true;
            }
            ui.label(
                RichText::new(t("audio.editing_snapshot"))
                    .color(theme::SELECTION)
                    .small(),
            );
        }
        changed
    }

    fn set_editing(&mut self, editor: &mut Editor, snapshot: Option<String>) {
        if let Some(mut audio) = editor.tree().world().resource_mut::<Audio>() {
            if let Some(old) = &self.editing {
                audio.set_snapshot(old, false);
            }
            if let Some(new) = &snapshot {
                audio.set_snapshot(new, true);
            }
        }
        self.editing = snapshot;
    }

    /// The values a strip shows for `name`.
    fn values(&self, mixer: &MixerSettings, name: &str, kind: Kind) -> StripValues {
        let mut v = match kind {
            Kind::Master => StripValues {
                volume_db: mixer.master.volume_db,
                mute: false,
                effects: mixer.master.effects.clone(),
                sends: Vec::new(),
            },
            Kind::Bus => {
                let b = mixer.buses.iter().find(|b| b.name == name);
                StripValues {
                    volume_db: b.map_or(0.0, |b| b.volume_db),
                    mute: b.is_some_and(|b| b.mute),
                    effects: b.map(|b| b.effects.clone()).unwrap_or_default(),
                    sends: b.map(|b| b.sends.clone()).unwrap_or_default(),
                }
            }
            Kind::Return => {
                let r = mixer.returns.iter().find(|r| r.name == name);
                StripValues {
                    volume_db: r.map_or(0.0, |r| r.volume_db),
                    mute: r.is_some_and(|r| r.mute),
                    effects: r.map(|r| r.effects.clone()).unwrap_or_default(),
                    sends: Vec::new(),
                }
            }
        };
        if let Some(o) = self.override_of(mixer, name) {
            if let Some(volume) = o.volume_db {
                v.volume_db = volume;
            }
            if let Some(mute) = o.mute {
                v.mute = mute;
            }
            if let Some(effects) = &o.effects {
                v.effects = effects.clone();
            }
            if let Some(sends) = &o.sends {
                v.sends = sends.clone();
            }
        }
        v
    }

    fn override_of<'m>(&self, mixer: &'m MixerSettings, bus: &str) -> Option<&'m BusOverride> {
        let snapshot = self.editing.as_ref()?;
        mixer
            .snapshots
            .iter()
            .find(|s| &s.name == snapshot)?
            .buses
            .iter()
            .find(|o| o.bus == bus)
    }

    /// Writes what a strip changed: into the bus, or into the edited snapshot (only the
    /// parts that changed become overrides).
    fn store(
        &self,
        mixer: &mut MixerSettings,
        name: &str,
        kind: Kind,
        before: &StripValues,
        after: StripValues,
    ) {
        if let Some(snapshot) = &self.editing {
            let Some(s) = mixer.snapshots.iter_mut().find(|s| &s.name == snapshot) else {
                return;
            };
            let o = match s.buses.iter().position(|o| o.bus == name) {
                Some(i) => &mut s.buses[i],
                None => {
                    s.buses.push(BusOverride {
                        bus: name.to_owned(),
                        ..BusOverride::default()
                    });
                    s.buses.last_mut().expect("just pushed")
                }
            };
            if after.volume_db != before.volume_db {
                o.volume_db = Some(after.volume_db);
            }
            if after.mute != before.mute {
                o.mute = Some(after.mute);
            }
            if after.effects != before.effects {
                o.effects = Some(after.effects);
            }
            if after.sends != before.sends {
                o.sends = Some(after.sends);
            }
            return;
        }
        match kind {
            Kind::Master => {
                mixer.master.volume_db = after.volume_db;
                mixer.master.effects = after.effects;
            }
            Kind::Bus => {
                if let Some(b) = mixer.buses.iter_mut().find(|b| b.name == name) {
                    b.volume_db = after.volume_db;
                    b.mute = after.mute;
                    b.effects = after.effects;
                    b.sends = after.sends;
                }
            }
            Kind::Return => {
                if let Some(r) = mixer.returns.iter_mut().find(|r| r.name == name) {
                    r.volume_db = after.volume_db;
                    r.mute = after.mute;
                    r.effects = after.effects;
                }
            }
        }
    }

    fn strip(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        name: &str,
        kind: Kind,
        depth: usize,
        clock: f64,
    ) -> bool {
        let before = self.values(&editor.mixer, name, kind);
        let mut v = before.clone();
        let mut structural = false;
        let overridden = self.override_of(&editor.mixer, name).is_some();
        let accent = match kind {
            Kind::Master => theme::SELECTION,
            Kind::Bus => theme::ACCENT,
            Kind::Return => theme::PLAY,
        };
        let frame = egui::Frame::NONE
            .fill(theme::BG_1)
            .stroke(egui::Stroke::new(
                1.0,
                if overridden {
                    theme::SELECTION
                } else {
                    theme::BORDER
                },
            ))
            .corner_radius(4)
            .inner_margin(egui::Margin::same(6));
        frame.show(ui, |ui| {
            ui.set_width(STRIP_WIDTH);
            ui.vertical(|ui| {
                // Header: a color band, the name, where it goes.
                let (band, _) =
                    ui.allocate_exact_size(Vec2::new(STRIP_WIDTH, 3.0), egui::Sense::hover());
                ui.painter().rect_filled(band, 1.0, accent);
                structural |= self.header(ui, editor, name, kind, depth);

                // Inserts and sends, in the same height on every strip so the faders line up.
                ui.allocate_ui_with_layout(
                    Vec2::new(STRIP_WIDTH, ROUTING_HEIGHT),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_height(ROUTING_HEIGHT);
                        egui::ScrollArea::vertical()
                            .id_salt(("strip_routing", name))
                            .max_height(ROUTING_HEIGHT)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                // Inserts.
                                ui.label(
                                    RichText::new(t("audio.inserts"))
                                        .color(theme::TEXT_DIM)
                                        .small(),
                                );
                                let mut remove = None;
                                let mut swap = None;
                                for (i, effect) in v.effects.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
                                        let power = if effect.bypass {
                                            theme::BG_4
                                        } else {
                                            theme::PLAY
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(icon::POWER).color(power),
                                                )
                                                .small()
                                                .frame(false),
                                            )
                                            .on_hover_text(t("audio.bypass"))
                                            .clicked()
                                        {
                                            effect.bypass = !effect.bypass;
                                        }
                                        let selected = self
                                            .selected
                                            .as_ref()
                                            .is_some_and(|(b, e)| b == name && *e == i);
                                        let text = widgets::effect_name(effect.kind.name());
                                        let response = ui.add_sized(
                                            [STRIP_WIDTH - 44.0, 18.0],
                                            egui::Button::new(RichText::new(text).small())
                                                .selected(selected),
                                        );
                                        if response.clicked() {
                                            self.selected = Some((name.to_owned(), i));
                                            self.eq_band = None;
                                        }
                                        response.context_menu(|ui| {
                                            if ui
                                                .button(format!(
                                                    "{}  {}",
                                                    icon::ARROW_UP,
                                                    t("audio.move_up")
                                                ))
                                                .clicked()
                                            {
                                                swap = Some((i, i.saturating_sub(1)));
                                                ui.close();
                                            }
                                            if ui
                                                .button(format!(
                                                    "{}  {}",
                                                    icon::ARROW_DOWN,
                                                    t("audio.move_down")
                                                ))
                                                .clicked()
                                            {
                                                swap = Some((i, i + 1));
                                                ui.close();
                                            }
                                        });
                                        if ui
                                            .add(
                                                egui::Button::new(RichText::new(icon::X).small())
                                                    .frame(false),
                                            )
                                            .on_hover_text(t("audio.remove_effect"))
                                            .clicked()
                                        {
                                            remove = Some(i);
                                        }
                                    });
                                }
                                if let Some((a, b)) = swap {
                                    if b < v.effects.len() && a != b {
                                        v.effects.swap(a, b);
                                    }
                                }
                                if let Some(i) = remove {
                                    v.effects.remove(i);
                                    self.selected = None;
                                }
                                ui.menu_button(
                                    RichText::new(format!(
                                        "{}  {}",
                                        icon::PLUS,
                                        t("audio.add_effect")
                                    ))
                                    .small(),
                                    |ui| {
                                        for kind in EFFECT_KINDS {
                                            if ui.button(widgets::effect_name(kind)).clicked() {
                                                if let Some(effect) = EffectKind::from_name(kind) {
                                                    v.effects.push(EffectDesc::new(effect));
                                                    self.selected = Some((
                                                        name.to_owned(),
                                                        v.effects.len() - 1,
                                                    ));
                                                }
                                                ui.close();
                                            }
                                        }
                                    },
                                );

                                // Sends.
                                if kind == Kind::Bus {
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(t("audio.sends"))
                                            .color(theme::TEXT_DIM)
                                            .small(),
                                    );
                                    let mut remove = None;
                                    for (i, send) in v.sends.iter_mut().enumerate() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 3.0;
                                            ui.label(RichText::new(&send.to).small());
                                            ui.add(
                                                egui::DragValue::new(&mut send.level_db)
                                                    .range(-80.0..=6.0)
                                                    .speed(0.2)
                                                    .suffix(" dB")
                                                    .max_decimals(1),
                                            );
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(icon::X).small(),
                                                    )
                                                    .frame(false),
                                                )
                                                .clicked()
                                            {
                                                remove = Some(i);
                                            }
                                        });
                                    }
                                    if let Some(i) = remove {
                                        v.sends.remove(i);
                                        structural = true;
                                    }
                                    let free: Vec<String> = editor
                                        .mixer
                                        .returns
                                        .iter()
                                        .map(|r| r.name.clone())
                                        .filter(|r| !v.sends.iter().any(|s| &s.to == r))
                                        .collect();
                                    if !free.is_empty() {
                                        ui.menu_button(
                                            RichText::new(format!(
                                                "{}  {}",
                                                icon::PLUS,
                                                t("audio.add_send")
                                            ))
                                            .small(),
                                            |ui| {
                                                for r in free {
                                                    if ui.button(&r).clicked() {
                                                        v.sends.push(SendDesc {
                                                            to: r,
                                                            level_db: -12.0,
                                                        });
                                                        structural = true;
                                                        ui.close();
                                                    }
                                                }
                                            },
                                        );
                                    }
                                }
                            });
                    },
                );
                ui.add_space(4.0);
                // Meter and fader.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let meter = self.meters.entry(name.to_owned()).or_default();
                    let response = widgets::meter(ui, meter, Vec2::new(26.0, FADER_HEIGHT));
                    if response.clicked() {
                        meter.reset_clip();
                    }
                    widgets::fader(
                        ui,
                        &mut v.volume_db,
                        Vec2::new(STRIP_WIDTH - 40.0, FADER_HEIGHT),
                    );
                });
                ui.horizontal(|ui| {
                    let meter = self.meters.entry(name.to_owned()).or_default();
                    let clip = meter.clipping(clock);
                    let peak = RichText::new(widgets::format_db(meter.peak_db()))
                        .small()
                        .monospace()
                        .color(if clip { theme::ERROR } else { theme::TEXT_DIM });
                    if ui
                        .add(egui::Label::new(peak).sense(egui::Sense::click()))
                        .on_hover_text(t("audio.clip_hint"))
                        .clicked()
                    {
                        meter.reset_clip();
                    }
                    ui.add(
                        egui::DragValue::new(&mut v.volume_db)
                            .range(-80.0..=widgets::FADER_MAX_DB)
                            .speed(0.1)
                            .suffix(" dB")
                            .max_decimals(1),
                    );
                });
                // Mute and solo.
                if kind != Kind::Master {
                    ui.horizontal(|ui| {
                        let mute =
                            egui::Button::new(RichText::new("M").strong().color(if v.mute {
                                theme::BG_0
                            } else {
                                theme::TEXT
                            }))
                            .fill(if v.mute { theme::WARN } else { theme::BG_3 })
                            .min_size(Vec2::new(28.0, 20.0));
                        if ui.add(mute).on_hover_text(t("audio.mute")).clicked() {
                            v.mute = !v.mute;
                        }
                        if kind == Kind::Bus {
                            let solo = editor
                                .mixer
                                .buses
                                .iter()
                                .find(|b| b.name == name)
                                .is_some_and(|b| b.solo);
                            let button =
                                egui::Button::new(RichText::new("S").strong().color(if solo {
                                    theme::BG_0
                                } else {
                                    theme::TEXT
                                }))
                                .fill(if solo { theme::PLAY } else { theme::BG_3 })
                                .min_size(Vec2::new(28.0, 20.0));
                            if ui.add(button).on_hover_text(t("audio.solo")).clicked() {
                                if let Some(b) =
                                    editor.mixer.buses.iter_mut().find(|b| b.name == name)
                                {
                                    b.solo = !b.solo;
                                    structural = true;
                                }
                            }
                        }
                        if overridden
                            && ui
                                .add(egui::Button::new(
                                    RichText::new(icon::ARROW_COUNTER_CLOCKWISE).small(),
                                ))
                                .on_hover_text(t("audio.clear_override"))
                                .clicked()
                        {
                            if let Some(s) = self.editing.as_ref().and_then(|e| {
                                editor.mixer.snapshots.iter_mut().find(|s| &s.name == e)
                            }) {
                                s.buses.retain(|o| o.bus != name);
                                structural = true;
                            }
                        }
                    });
                }
            });
        });
        let changed = v != before;
        if changed {
            self.store(&mut editor.mixer, name, kind, &before, v);
        }
        changed || structural
    }

    /// Name (double-click to rename), where it goes, and its menu.
    fn header(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        name: &str,
        kind: Kind,
        depth: usize,
    ) -> bool {
        let mut changed = false;
        if let Some((old, text)) = &mut self.rename {
            if old == name {
                let edit = ui.add(egui::TextEdit::singleline(text).desired_width(STRIP_WIDTH));
                edit.request_focus();
                if edit.lost_focus() {
                    let (old, new) = (old.clone(), text.trim().to_owned());
                    self.rename = None;
                    if !new.is_empty() && new != old && !editor.mixer.bus_names().any(|n| n == new)
                    {
                        rename(&mut editor.mixer, &old, &new);
                        if let Some((bus, _)) = &mut self.selected {
                            if *bus == old {
                                *bus = new.clone();
                            }
                        }
                        changed = true;
                    }
                }
                return changed;
            }
        }
        let indent = "› ".repeat(depth);
        let title = egui::Label::new(RichText::new(format!("{indent}{name}")).strong())
            .sense(egui::Sense::click());
        let response = ui.add(title);
        if response.double_clicked() && kind != Kind::Master {
            self.rename = Some((name.to_owned(), name.to_owned()));
        }
        let route = match kind {
            Kind::Master => t("audio.to_output"),
            Kind::Return => t("audio.return_bus"),
            Kind::Bus => format!(
                "→ {}",
                editor
                    .mixer
                    .buses
                    .iter()
                    .find(|b| b.name == name)
                    .map_or(MASTER, |b| b.parent_name())
            ),
        };
        ui.label(RichText::new(route).small().color(theme::TEXT_DIM));
        if kind == Kind::Master {
            return false;
        }
        response.context_menu(|ui| {
            if ui
                .button(format!("{}  {}", icon::PENCIL_SIMPLE, t("audio.rename")))
                .clicked()
            {
                self.rename = Some((name.to_owned(), name.to_owned()));
                ui.close();
            }
            if kind == Kind::Bus {
                ui.menu_button(
                    format!("{}  {}", icon::ARROW_BEND_DOWN_RIGHT, t("audio.output_to")),
                    |ui| {
                        let mut targets: Vec<String> = vec![MASTER.to_owned()];
                        targets.extend(
                            editor
                                .mixer
                                .buses
                                .iter()
                                .map(|b| b.name.clone())
                                .filter(|b| b != name && !feeds(&editor.mixer, b, name)),
                        );
                        for target in targets {
                            if ui.button(&target).clicked() {
                                if let Some(b) =
                                    editor.mixer.buses.iter_mut().find(|b| b.name == name)
                                {
                                    b.parent = (target != MASTER).then_some(target);
                                    changed = true;
                                }
                                ui.close();
                            }
                        }
                    },
                );
            }
            ui.separator();
            if ui
                .button(
                    RichText::new(format!("{}  {}", icon::TRASH, t("audio.delete_bus")))
                        .color(theme::ERROR),
                )
                .clicked()
            {
                delete(&mut editor.mixer, name);
                self.selected = None;
                changed = true;
                ui.close();
            }
        });
        response.on_hover_text(t("audio.bus_hint"));
        changed
    }

    /// The selected effect's editor.
    fn effect_editor(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        levels: &HashMap<String, std::sync::Arc<ab_engine::audio::Levels>>,
        delta: f32,
    ) -> bool {
        let Some((bus, index)) = self.selected.clone() else {
            crate::panels::empty_hint(ui, icon::SLIDERS, &t("audio.pick_effect"));
            return false;
        };
        let kind = if bus == MASTER {
            Kind::Master
        } else if editor.mixer.returns.iter().any(|r| r.name == bus) {
            Kind::Return
        } else {
            Kind::Bus
        };
        let before = self.values(&editor.mixer, &bus, kind);
        let Some(effect) = before.effects.get(index).cloned() else {
            self.selected = None;
            return false;
        };
        let mut effect = effect;
        let buses: Vec<String> = editor
            .mixer
            .bus_names()
            .filter(|n| *n != bus)
            .map(str::to_owned)
            .collect();
        let bus_levels = levels.get(&bus);
        let reduction = self.meters.get(&bus).map_or(0.0, MeterState::reduction_db);
        egui::ScrollArea::vertical()
            .id_salt("mixer_effect")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(format!(
                        "{}  {}",
                        widgets::effect_name(effect.kind.name()),
                        RichText::new(format!("· {bus}"))
                            .color(theme::TEXT_DIM)
                            .text()
                    ));
                    ui.checkbox(&mut effect.bypass, t("audio.bypass"));
                });
                match &mut effect.kind {
                    EffectKind::Eq(eq) => {
                        if let Some(levels) = bus_levels {
                            self.spectrum.update(levels, delta);
                        }
                        let rate = bus_levels.map_or(48_000, |l| l.sample_rate()) as f32;
                        let spectrum = bus_levels.map(|_| &self.spectrum);
                        widgets::eq_graph(ui, eq, &mut self.eq_band, spectrum, rate, 240.0);
                        ui.add_space(4.0);
                        widgets::eq_bands(ui, eq, &mut self.eq_band);
                    }
                    EffectKind::Compressor(c) => {
                        ui.horizontal_top(|ui| {
                            widgets::compressor_graph(ui, c, reduction, 180.0);
                            ui.vertical(|ui| {
                                let mut wrapped = EffectDesc {
                                    bypass: false,
                                    kind: EffectKind::Compressor(c.clone()),
                                };
                                if widgets::effect_params(
                                    ui,
                                    &mut wrapped,
                                    &buses,
                                    "compressor_params",
                                ) {
                                    if let EffectKind::Compressor(new) = wrapped.kind {
                                        *c = new;
                                    }
                                }
                            });
                        });
                    }
                    _ => {
                        widgets::effect_params(ui, &mut effect, &buses, "effect_params");
                    }
                }
            });
        if effect != before.effects[index] {
            let mut after = before.clone();
            after.effects[index] = effect;
            self.store(&mut editor.mixer, &bus, kind, &before, after);
            return true;
        }
        false
    }
}

/// Strips in order: the master, the buses as a tree (parents before children), the returns.
fn order(mixer: &MixerSettings) -> Vec<(String, Kind, usize)> {
    let mut out = vec![(MASTER.to_owned(), Kind::Master, 0)];
    fn children(
        mixer: &MixerSettings,
        parent: &str,
        depth: usize,
        out: &mut Vec<(String, Kind, usize)>,
    ) {
        for bus in mixer.buses.iter().filter(|b| b.parent_name() == parent) {
            if out.iter().any(|(n, _, _)| *n == bus.name) {
                continue;
            }
            out.push((bus.name.clone(), Kind::Bus, depth));
            children(mixer, &bus.name, depth + 1, out);
        }
    }
    children(mixer, MASTER, 0, &mut out);
    // Buses under a missing parent still show.
    for bus in &mixer.buses {
        if !out.iter().any(|(n, _, _)| *n == bus.name) {
            out.push((bus.name.clone(), Kind::Bus, 0));
        }
    }
    out.extend(
        mixer
            .returns
            .iter()
            .map(|r| (r.name.clone(), Kind::Return, 0)),
    );
    out
}

/// Whether `bus` feeds `target` (directly or through others).
fn feeds(mixer: &MixerSettings, bus: &str, target: &str) -> bool {
    let mut at = bus.to_owned();
    for _ in 0..mixer.buses.len() + 1 {
        let Some(b) = mixer.buses.iter().find(|b| b.name == at) else {
            return false;
        };
        let parent = b.parent_name();
        if parent == target {
            return true;
        }
        at = parent.to_owned();
    }
    false
}

fn unique_name(mixer: &MixerSettings, base: &str) -> String {
    (1..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !mixer.bus_names().any(|b| b == n))
        .expect("an unused name")
}

/// Renames a bus or return everywhere it is named.
fn rename(mixer: &mut MixerSettings, old: &str, new: &str) {
    for bus in &mut mixer.buses {
        if bus.name == old {
            bus.name = new.to_owned();
        }
        if bus.parent.as_deref() == Some(old) {
            bus.parent = Some(new.to_owned());
        }
        for send in &mut bus.sends {
            if send.to == old {
                send.to = new.to_owned();
            }
        }
    }
    for r in &mut mixer.returns {
        if r.name == old {
            r.name = new.to_owned();
        }
    }
    let chains = mixer
        .master
        .effects
        .iter_mut()
        .chain(mixer.buses.iter_mut().flat_map(|b| b.effects.iter_mut()))
        .chain(mixer.returns.iter_mut().flat_map(|r| r.effects.iter_mut()));
    for effect in chains {
        if let EffectKind::Compressor(c) = &mut effect.kind {
            if c.sidechain.as_deref() == Some(old) {
                c.sidechain = Some(new.to_owned());
            }
        }
    }
    for s in &mut mixer.snapshots {
        for o in &mut s.buses {
            if o.bus == old {
                o.bus = new.to_owned();
            }
        }
    }
}

/// Deletes a bus (its children move to its parent) or a return (sends to it go).
fn delete(mixer: &mut MixerSettings, name: &str) {
    let parent = mixer
        .buses
        .iter()
        .find(|b| b.name == name)
        .and_then(|b| b.parent.clone());
    mixer.buses.retain(|b| b.name != name);
    mixer.returns.retain(|r| r.name != name);
    for bus in &mut mixer.buses {
        if bus.parent.as_deref() == Some(name) {
            bus.parent = parent.clone();
        }
        bus.sends.retain(|s| s.to != name);
    }
    for s in &mut mixer.snapshots {
        s.buses.retain(|o| o.bus != name);
    }
    let chains = mixer
        .master
        .effects
        .iter_mut()
        .chain(mixer.buses.iter_mut().flat_map(|b| b.effects.iter_mut()))
        .chain(mixer.returns.iter_mut().flat_map(|r| r.effects.iter_mut()));
    for effect in chains {
        if let EffectKind::Compressor(c) = &mut effect.kind {
            if c.sidechain.as_deref() == Some(name) {
                c.sidechain = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixer() -> MixerSettings {
        let mut m = MixerSettings::default();
        m.buses.push(BusDesc {
            parent: Some("SFX".into()),
            ..BusDesc::new("Footsteps")
        });
        m.buses[0]
            .effects
            .push(EffectDesc::new(EffectKind::Compressor(
                ab_engine::audio::effect::CompressorSettings {
                    sidechain: Some("Voice".into()),
                    ..Default::default()
                },
            )));
        m
    }

    #[test]
    fn strips_list_buses_as_a_tree_between_the_master_and_the_returns() {
        let names: Vec<_> = order(&mixer())
            .into_iter()
            .map(|(n, _, d)| (n, d))
            .collect();
        let at = |n: &str| names.iter().position(|(x, _)| x == n).unwrap();
        assert_eq!(at("Master"), 0);
        assert_eq!(at("Footsteps"), at("SFX") + 1);
        assert_eq!(names[at("Footsteps")].1, 1);
        assert_eq!(names.last().unwrap().0, "Reverb");
    }

    #[test]
    fn renaming_follows_parents_sidechains_and_snapshots() {
        let mut m = mixer();
        m.snapshots.push(SnapshotDesc {
            name: "Pause".into(),
            buses: vec![BusOverride {
                bus: "Voice".into(),
                volume_db: Some(-10.0),
                ..Default::default()
            }],
            ..Default::default()
        });
        rename(&mut m, "SFX", "Effects");
        rename(&mut m, "Voice", "Dialogue");
        assert_eq!(m.buses.last().unwrap().parent.as_deref(), Some("Effects"));
        let EffectKind::Compressor(c) = &m.buses[0].effects[0].kind else {
            unreachable!()
        };
        assert_eq!(c.sidechain.as_deref(), Some("Dialogue"));
        assert_eq!(m.snapshots[0].buses[0].bus, "Dialogue");
        assert!(m.problems().is_empty(), "{:?}", m.problems());
    }

    #[test]
    fn deleting_a_bus_keeps_its_children_and_drops_what_named_it() {
        let mut m = mixer();
        delete(&mut m, "SFX");
        assert_eq!(
            m.buses
                .iter()
                .find(|b| b.name == "Footsteps")
                .unwrap()
                .parent,
            None
        );
        delete(&mut m, "Reverb");
        assert!(m.buses.iter().all(|b| b.sends.is_empty()));
        delete(&mut m, "Voice");
        let EffectKind::Compressor(c) = &m.buses[0].effects[0].kind else {
            unreachable!()
        };
        assert_eq!(c.sidechain, None);
        assert!(m.problems().is_empty(), "{:?}", m.problems());
    }

    #[test]
    fn a_bus_cannot_be_routed_into_its_own_children() {
        let m = mixer();
        assert!(feeds(&m, "Footsteps", "SFX"));
        assert!(!feeds(&m, "SFX", "Footsteps"));
    }

    #[test]
    fn snapshot_edits_become_overrides_of_only_what_changed() {
        let mut panel = AudioMixerPanel {
            editing: Some("Pause".into()),
            ..Default::default()
        };
        let mut m = mixer();
        m.snapshots.push(SnapshotDesc {
            name: "Pause".into(),
            ..Default::default()
        });
        let before = panel.values(&m, "Music", Kind::Bus);
        let mut after = before.clone();
        after.volume_db = -20.0;
        panel.store(&mut m, "Music", Kind::Bus, &before, after);
        let o = &m.snapshots[0].buses[0];
        assert_eq!(
            (o.bus.as_str(), o.volume_db, o.mute),
            ("Music", Some(-20.0), None)
        );
        assert_eq!(m.buses[0].volume_db, 0.0, "the bus itself is unchanged");
        assert_eq!(panel.values(&m, "Music", Kind::Bus).volume_db, -20.0);
        panel.editing = None;
        assert_eq!(panel.values(&m, "Music", Kind::Bus).volume_db, 0.0);
    }
}
