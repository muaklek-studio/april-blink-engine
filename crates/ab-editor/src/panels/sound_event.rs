//! The Sound Event editor: a `*.absound` asset's clips (cut on their waveform), how they
//! are picked and varied, its bus and voice limits and how it sounds in 3D. Play hears it
//! as the game will: through the mixer, a new pick and variation each time.

use crate::audio_widgets as widgets;
use crate::i18n::{t, tf};
use crate::project::AssetKind;
use crate::state::Editor;
use crate::theme;
use ab_engine::asset::AssetServer;
use ab_engine::audio::clip::{peak_db, waveform};
use ab_engine::audio::{
    Attenuation, Audio, AudioStream, ClipDesc, PickMode, Rolloff, SoundEvent, SoundEventDesc,
    StealMode, VoiceId, MASTER,
};
use ab_engine::core::Vec3;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use std::collections::HashMap;
use std::time::Instant;

/// Columns the waveforms are drawn with.
const WAVEFORM_COLUMNS: usize = 1200;

/// A loaded clip file: for playing, and its shape for drawing.
struct Loaded {
    stream: AudioStream,
    peaks: Vec<[f32; 2]>,
    duration: f32,
    peak_db: Option<f32>,
}

#[derive(Default)]
pub struct SoundEventEditor {
    /// The asset being edited.
    path: Option<String>,
    desc: SoundEventDesc,
    saved: SoundEventDesc,
    clip: Option<usize>,
    files: HashMap<String, Result<Loaded, String>>,
    /// What plays, when it started, and the clip's start in its file (for the playhead).
    playing: Option<(VoiceId, Instant, f32, Option<usize>)>,
    /// How far in front of the listener a 3D event is heard when previewed, m.
    preview_distance: f32,
    error: Option<String>,
}

impl SoundEventEditor {
    /// Opens `path` (an asset path), replacing what was open.
    pub fn open(&mut self, editor: &Editor, path: &str) {
        let full = editor.project().join(path);
        let loaded = std::fs::read_to_string(&full)
            .map_err(|e| e.to_string())
            .and_then(|text| SoundEventDesc::from_toml(&text));
        match loaded {
            Ok(desc) => {
                self.path = Some(path.to_owned());
                self.saved = desc.clone();
                self.desc = desc;
                self.clip = (!self.desc.clips.is_empty()).then_some(0);
                self.error = None;
            }
            Err(err) => {
                self.path = Some(path.to_owned());
                self.error = Some(format!("{path}: {err}"));
            }
        }
    }

    pub fn is_modified(&self) -> bool {
        self.path.is_some() && self.error.is_none() && self.desc != self.saved
    }

    fn save(&mut self, editor: &mut Editor) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let full = editor.project().join(&path);
        match editor.write_project_file(&full, self.desc.to_toml()) {
            Ok(()) => {
                self.saved = self.desc.clone();
                // Scenes using it load the new version.
                if let Some(mut assets) = editor.tree().world().resource_mut::<AssetServer>() {
                    assets.evict(&path);
                }
                editor.info(tf("audio.event_saved", &[("path", &path)]));
            }
            Err(err) => editor.fail(format!("{}: {err}", full.display())),
        }
    }

    fn load(&mut self, editor: &Editor, path: &str) {
        if self.files.contains_key(path) {
            return;
        }
        let loaded = editor
            .tree()
            .world()
            .resource_mut::<AssetServer>()
            .ok_or_else(|| "no asset server".to_owned())
            .and_then(|mut assets| assets.load_audio(path).map_err(|e| e.to_string()))
            .and_then(|stream| {
                // Streamed music is decoded whole once, to draw it.
                let decoded = stream.decoded().map_err(|e| e.to_string())?;
                Ok(Loaded {
                    peaks: waveform(&decoded, WAVEFORM_COLUMNS).unwrap_or_default(),
                    duration: decoded.duration().as_secs_f32(),
                    peak_db: peak_db(&decoded),
                    stream,
                })
            });
        self.files.insert(path.to_owned(), loaded);
    }

    /// The event as the game would load it now (unsaved edits included).
    fn event(&mut self, editor: &Editor, only: Option<usize>) -> SoundEvent {
        for clip in self.desc.clips.clone() {
            self.load(editor, &clip.path);
        }
        let mut desc = self.desc.clone();
        if let Some(i) = only {
            desc.clips = desc.clips.get(i).cloned().into_iter().collect();
            desc.mode = PickMode::Sequence;
        }
        let streams = desc
            .clips
            .iter()
            .map(|c| {
                self.files
                    .get(&c.path)
                    .and_then(|f| f.as_ref().ok())
                    .map(|f| f.stream.clone())
            })
            .collect();
        SoundEvent::new(self.path.as_deref().unwrap_or_default(), desc, streams)
    }

    fn play(&mut self, editor: &Editor, only: Option<usize>) {
        self.stop(editor);
        let event = self.event(editor, only);
        let Some(mut audio) = editor.tree().world().resource_mut::<Audio>() else {
            return;
        };
        let id = if self.desc.spatial.is_some() {
            let (position, rotation) = audio.listener_transform();
            let ahead = rotation * Vec3::NEG_Z * self.preview_distance.max(0.0);
            audio.play_event_at(&event, position + ahead)
        } else {
            audio.play_event(&event)
        };
        match id {
            Some(id) => {
                let clip = only.or(self.clip);
                let start = clip
                    .and_then(|i| self.desc.clips.get(i))
                    .map_or(0.0, |c| c.edit.start);
                self.playing = Some((id, Instant::now(), start, clip));
            }
            None => self.error = Some(t("audio.not_played")),
        }
    }

    fn stop(&mut self, editor: &Editor) {
        if let Some((id, ..)) = self.playing.take() {
            if let Some(mut audio) = editor.tree().world().resource_mut::<Audio>() {
                audio.stop_voice(id, 0.05);
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        if self.preview_distance == 0.0 {
            self.preview_distance = 3.0;
        }
        if self.path.is_none() {
            self.pick_event(ui, editor);
            return;
        }
        let playing = self.playing.as_ref().is_some_and(|(id, ..)| {
            editor
                .tree()
                .world()
                .resource::<Audio>()
                .is_some_and(|a| a.is_voice_playing(*id))
        });
        if playing {
            ui.ctx().request_repaint();
        } else {
            self.playing = None;
        }
        self.toolbar(ui, editor);
        if let Some(error) = &self.error {
            ui.label(RichText::new(format!("{}  {error}", icon::WARNING)).color(theme::WARN));
        }
        ui.separator();
        let ctrl_s = ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            ))
        });
        if ctrl_s && self.is_modified() {
            self.save(editor);
        }
        let space =
            ui.input(|i| i.key_pressed(egui::Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        if space {
            if playing {
                self.stop(editor);
            } else {
                self.play(editor, None);
            }
        }

        crate::tool_window::side(
            ui,
            "sound_event_clips",
            true,
            260.0,
            icon::LIST,
            &t("audio.clips"),
            |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.clips(ui, editor));
            },
        );
        crate::tool_window::side(
            ui,
            "sound_event_settings",
            false,
            340.0,
            icon::SLIDERS,
            &t("audio.event_settings"),
            |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui, editor));
            },
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| self.clip_editor(ui, editor));
    }

    fn pick_event(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        crate::panels::empty_hint(ui, icon::WAVEFORM, &t("audio.pick_event"));
        let events: Vec<String> = editor
            .assets
            .of_kind(AssetKind::SoundEvent)
            .map(|a| a.path.clone())
            .collect();
        for path in events {
            if ui.button(format!("{}  {path}", icon::WAVEFORM)).clicked() {
                self.open(editor, &path);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        ui.horizontal(|ui| {
            let path = self.path.clone().unwrap_or_default();
            let mark = if self.is_modified() { " •" } else { "" };
            ui.label(RichText::new(format!("{}  {path}{mark}", icon::WAVEFORM)).strong());
            ui.separator();
            if ui
                .add_enabled(
                    self.is_modified(),
                    egui::Button::new(format!("{}  {}", icon::FLOPPY_DISK, t("audio.save"))),
                )
                .clicked()
            {
                self.save(editor);
            }
            if ui
                .add_enabled(
                    self.is_modified(),
                    egui::Button::new(format!(
                        "{}  {}",
                        icon::ARROW_COUNTER_CLOCKWISE,
                        t("audio.revert")
                    )),
                )
                .clicked()
            {
                self.desc = self.saved.clone();
            }
            ui.separator();
            let can_play = !self.desc.clips.is_empty();
            if ui
                .add_enabled(
                    can_play,
                    egui::Button::new(format!("{}  {}", icon::PLAY, t("audio.play_event"))),
                )
                .on_hover_text(t("audio.play_event_hint"))
                .clicked()
            {
                self.play(editor, None);
            }
            if ui
                .button(format!("{}  {}", icon::STOP, t("audio.stop")))
                .clicked()
            {
                self.stop(editor);
                if let Some(mut audio) = editor.tree().world().resource_mut::<Audio>() {
                    audio.stop_all();
                }
            }
            if self.desc.spatial.is_some() {
                ui.label(RichText::new(t("audio.preview_distance")).color(theme::TEXT_DIM));
                ui.add(
                    egui::DragValue::new(&mut self.preview_distance)
                        .range(0.0..=1000.0)
                        .speed(0.1)
                        .suffix(" m"),
                );
            }
            ui.separator();
            if ui
                .button(format!("{}  {}", icon::FOLDER_OPEN, t("audio.open_other")))
                .clicked()
            {
                self.stop(editor);
                self.path = None;
            }
        });
    }

    fn clips(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let mut remove = None;
        let mut play = None;
        let total: f32 = self.desc.clips.iter().map(|c| c.weight.max(0.0)).sum();
        for (i, clip) in self.desc.clips.iter_mut().enumerate() {
            let selected = self.clip == Some(i);
            egui::Frame::NONE
                .fill(if selected { theme::BG_3 } else { theme::BG_1 })
                .corner_radius(3)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(icon::PLAY)
                            .on_hover_text(t("audio.play_clip"))
                            .clicked()
                        {
                            play = Some(i);
                        }
                        let file = clip
                            .path
                            .rsplit('/')
                            .next()
                            .unwrap_or(&clip.path)
                            .to_owned();
                        if ui
                            .selectable_label(selected, file)
                            .on_hover_text(&clip.path)
                            .clicked()
                        {
                            self.clip = Some(i);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(icon::X)
                                .on_hover_text(t("audio.remove_clip"))
                                .clicked()
                            {
                                remove = Some(i);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t("audio.weight"))
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                        ui.add(
                            egui::DragValue::new(&mut clip.weight)
                                .range(0.0..=100.0)
                                .speed(0.05)
                                .max_decimals(2),
                        );
                        if total > 0.0 {
                            let share = clip.weight.max(0.0) / total * 100.0;
                            ui.label(
                                RichText::new(format!("{share:.0}%"))
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                        if !clip.edit.is_identity() {
                            ui.label(RichText::new(icon::SCISSORS).small().color(theme::ACCENT))
                                .on_hover_text(t("audio.clip_edited"));
                        }
                    });
                });
        }
        if let Some(i) = play {
            self.clip = Some(i);
            self.play(editor, Some(i));
        }
        if let Some(i) = remove {
            self.desc.clips.remove(i);
            self.clip = (!self.desc.clips.is_empty()).then(|| i.min(self.desc.clips.len() - 1));
        }
        ui.add_space(4.0);
        let audio_files: Vec<String> = editor
            .assets
            .of_kind(AssetKind::Audio)
            .map(|a| a.path.clone())
            .collect();
        ui.menu_button(format!("{}  {}", icon::PLUS, t("audio.add_clip")), |ui| {
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    if audio_files.is_empty() {
                        ui.label(t("audio.no_sound_files"));
                    }
                    for path in audio_files {
                        if ui.button(&path).clicked() {
                            self.desc.clips.push(ClipDesc::new(&path));
                            self.clip = Some(self.desc.clips.len() - 1);
                            ui.close();
                        }
                    }
                });
        });
        ui.label(
            RichText::new(t("audio.clips_hint"))
                .small()
                .color(theme::TEXT_DIM),
        );
    }

    fn clip_editor(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let Some(index) = self.clip.filter(|&i| i < self.desc.clips.len()) else {
            crate::panels::empty_hint(ui, icon::SCISSORS, &t("audio.pick_clip"));
            return;
        };
        let path = self.desc.clips[index].path.clone();
        self.load(editor, &path);
        let playhead = self.playing.as_ref().and_then(|(_, started, start, clip)| {
            (*clip == Some(index)).then(|| start + started.elapsed().as_secs_f32())
        });
        let mut play_from = None;
        match self.files.get(&path) {
            Some(Ok(file)) => {
                let clip = &mut self.desc.clips[index];
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&path).strong());
                    ui.label(
                        RichText::new(format!(
                            "{:.2} s · {} {}",
                            file.duration,
                            t("audio.peak"),
                            file.peak_db
                                .map_or("?".to_owned(), |p| format!("{p:.1} dBFS"))
                        ))
                        .color(theme::TEXT_DIM),
                    );
                });
                let height = (ui.available_height() * 0.55).clamp(120.0, 320.0);
                let (_, clicked) = widgets::waveform_editor(
                    ui,
                    &file.peaks,
                    file.duration,
                    &mut clip.edit,
                    playhead.filter(|&p| p <= file.duration),
                    height,
                );
                play_from = clicked;
                ui.add_space(6.0);
                let e = &mut clip.edit;
                let max = file.duration;
                crate::panels::grid(ui, "clip_edit", |ui| {
                    crate::panels::label(ui, "Start");
                    ui.add(
                        egui::DragValue::new(&mut e.start)
                            .range(0.0..=max)
                            .speed(0.001)
                            .suffix(" s")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    crate::panels::label(ui, "End");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut e.end)
                                .range(0.0..=max)
                                .speed(0.001)
                                .suffix(" s")
                                .max_decimals(3),
                        );
                        ui.label(
                            RichText::new(t("audio.end_hint"))
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                    });
                    ui.end_row();
                    crate::panels::label(ui, "Fade in");
                    ui.add(
                        egui::DragValue::new(&mut e.fade_in)
                            .range(0.0..=max)
                            .speed(0.001)
                            .suffix(" s")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    crate::panels::label(ui, "Fade out");
                    ui.add(
                        egui::DragValue::new(&mut e.fade_out)
                            .range(0.0..=max)
                            .speed(0.001)
                            .suffix(" s")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    crate::panels::label(ui, "Volume");
                    ui.add(
                        egui::DragValue::new(&mut e.volume_db)
                            .range(-60.0..=24.0)
                            .speed(0.1)
                            .suffix(" dB")
                            .max_decimals(1),
                    );
                    ui.end_row();
                    crate::panels::label(ui, "Normalize");
                    ui.checkbox(&mut e.normalize, t("audio.normalize_hint"));
                    ui.end_row();
                    crate::panels::label(ui, "Loop start");
                    ui.add(
                        egui::DragValue::new(&mut e.loop_start)
                            .range(0.0..=max)
                            .speed(0.001)
                            .suffix(" s")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    crate::panels::label(ui, "Loop end");
                    ui.add(
                        egui::DragValue::new(&mut e.loop_end)
                            .range(0.0..=max)
                            .speed(0.001)
                            .suffix(" s")
                            .max_decimals(3),
                    );
                    ui.end_row();
                });
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::ARROW_COUNTER_CLOCKWISE,
                        t("audio.reset_cut")
                    ))
                    .clicked()
                {
                    *e = Default::default();
                }
                if file.stream.is_streaming() && e.normalize {
                    ui.label(
                        RichText::new(t("audio.normalize_streaming"))
                            .small()
                            .color(theme::WARN),
                    );
                }
            }
            Some(Err(err)) => {
                ui.label(
                    RichText::new(format!("{}  {path}: {err}", icon::WARNING)).color(theme::ERROR),
                );
            }
            None => {}
        }
        if let Some(at) = play_from {
            // Preview from the clicked point: the clip with its start moved there.
            let saved = self.desc.clips[index].edit.start;
            self.desc.clips[index].edit.start = at;
            self.play(editor, Some(index));
            self.desc.clips[index].edit.start = saved;
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let d = &mut self.desc;
        crate::panels::section(ui, "event_mix", icon::FADERS, "Mix", |ui| {
            crate::panels::grid(ui, "event_mix_grid", |ui| {
                crate::panels::label(ui, "Bus");
                let label = if d.bus.is_empty() {
                    MASTER.to_owned()
                } else {
                    d.bus.clone()
                };
                egui::ComboBox::from_id_salt("event_bus")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for bus in editor
                            .mixer
                            .bus_names()
                            .filter(|b| !editor.mixer.returns.iter().any(|r| r.name == *b))
                        {
                            let value = if bus == MASTER {
                                String::new()
                            } else {
                                bus.to_owned()
                            };
                            ui.selectable_value(&mut d.bus, value, bus);
                        }
                    });
                ui.end_row();
                crate::panels::label(ui, "Volume");
                ui.add(
                    egui::DragValue::new(&mut d.volume_db)
                        .range(-60.0..=24.0)
                        .speed(0.1)
                        .suffix(" dB"),
                );
                ui.end_row();
                crate::panels::label(ui, "Volume variation");
                ui.add(
                    egui::DragValue::new(&mut d.volume_random_db)
                        .range(0.0..=24.0)
                        .speed(0.05)
                        .prefix("± ")
                        .suffix(" dB"),
                );
                ui.end_row();
                crate::panels::label(ui, "Pitch");
                ui.add(
                    egui::DragValue::new(&mut d.pitch_semitones)
                        .range(-24.0..=24.0)
                        .speed(0.05)
                        .suffix(" st"),
                );
                ui.end_row();
                crate::panels::label(ui, "Pitch variation");
                ui.add(
                    egui::DragValue::new(&mut d.pitch_random_semitones)
                        .range(0.0..=12.0)
                        .speed(0.05)
                        .prefix("± ")
                        .suffix(" st"),
                );
                ui.end_row();
                crate::panels::label(ui, "Looping");
                ui.checkbox(&mut d.looping, "");
                ui.end_row();
            });
        });
        crate::panels::section(ui, "event_playback", icon::SHUFFLE, "Playback", |ui| {
            crate::panels::grid(ui, "event_playback_grid", |ui| {
                crate::panels::label(ui, "Pick");
                egui::ComboBox::from_id_salt("event_mode")
                    .selected_text(mode_name(d.mode))
                    .show_ui(ui, |ui| {
                        for mode in [
                            PickMode::RandomNoRepeat,
                            PickMode::Random,
                            PickMode::Shuffle,
                            PickMode::Sequence,
                        ] {
                            ui.selectable_value(&mut d.mode, mode, mode_name(mode));
                        }
                    });
                ui.end_row();
                crate::panels::label(ui, "Max voices");
                ui.add(egui::DragValue::new(&mut d.max_voices).range(0..=256))
                    .on_hover_text(t("audio.max_voices_event_hint"));
                ui.end_row();
                crate::panels::label(ui, "When full");
                egui::ComboBox::from_id_salt("event_steal")
                    .selected_text(steal_name(d.steal))
                    .show_ui(ui, |ui| {
                        for steal in [StealMode::Oldest, StealMode::Quietest, StealMode::None] {
                            ui.selectable_value(&mut d.steal, steal, steal_name(steal));
                        }
                    });
                ui.end_row();
                crate::panels::label(ui, "Priority");
                ui.add(egui::Slider::new(&mut d.priority, 0..=255))
                    .on_hover_text(t("audio.priority_hint"));
                ui.end_row();
                crate::panels::label(ui, "Cooldown");
                ui.add(
                    egui::DragValue::new(&mut d.cooldown)
                        .range(0.0..=10.0)
                        .speed(0.005)
                        .suffix(" s"),
                )
                .on_hover_text(t("audio.cooldown_hint"));
                ui.end_row();
            });
        });
        crate::panels::section(ui, "event_3d", icon::CUBE, "3D", |ui| {
            let mut spatial = d.spatial.is_some();
            if ui.checkbox(&mut spatial, t("audio.positional")).changed() {
                d.spatial = spatial.then(Attenuation::default);
            }
            if let Some(a) = &mut d.spatial {
                attenuation_editor(ui, a, "event_spatial");
            }
        });
    }
}

/// The settings of a positional sound, with its rolloff graph (shared with the Inspector).
pub fn attenuation_editor(ui: &mut egui::Ui, a: &mut Attenuation, id: &str) -> bool {
    let mut changed = false;
    crate::panels::grid(ui, id, |ui| {
        crate::panels::label(ui, "Full volume within");
        changed |= ui
            .add(
                egui::DragValue::new(&mut a.min_distance)
                    .range(0.0..=1000.0)
                    .speed(0.05)
                    .suffix(" m"),
            )
            .changed();
        ui.end_row();
        crate::panels::label(ui, "Silent beyond");
        changed |= ui
            .add(
                egui::DragValue::new(&mut a.max_distance)
                    .range(0.0..=10_000.0)
                    .speed(0.1)
                    .suffix(" m"),
            )
            .changed();
        ui.end_row();
        crate::panels::label(ui, "Rolloff");
        egui::ComboBox::from_id_salt((id, "rolloff"))
            .selected_text(rolloff_name(a.rolloff))
            .show_ui(ui, |ui| {
                for r in [
                    Rolloff::Inverse,
                    Rolloff::Linear,
                    Rolloff::Logarithmic,
                    Rolloff::Custom,
                ] {
                    if ui
                        .selectable_value(&mut a.rolloff, r, rolloff_name(r))
                        .changed()
                    {
                        changed = true;
                        if r == Rolloff::Custom && a.curve.is_empty() {
                            a.curve = vec![[0.0, 1.0], [0.3, 0.45], [1.0, 0.0]];
                        }
                    }
                }
            });
        ui.end_row();
        crate::panels::label(ui, "Spread");
        changed |= ui
            .add(egui::Slider::new(&mut a.spread, 0.0..=1.0).fixed_decimals(2))
            .on_hover_text(t("audio.spread_hint"))
            .changed();
        ui.end_row();
        crate::panels::label(ui, "Doppler");
        changed |= ui
            .add(egui::Slider::new(&mut a.doppler, 0.0..=2.0).fixed_decimals(2))
            .on_hover_text(t("audio.doppler_hint"))
            .changed();
        ui.end_row();
        crate::panels::label(ui, "Occlusion");
        changed |= ui
            .checkbox(&mut a.occlusion, "")
            .on_hover_text(t("audio.occlusion_hint"))
            .changed();
        ui.end_row();
    });
    changed |= widgets::attenuation_graph(ui, a, 120.0);
    changed
}

fn mode_name(mode: PickMode) -> String {
    t(match mode {
        PickMode::RandomNoRepeat => "audio.pick_random_no_repeat",
        PickMode::Random => "audio.pick_random",
        PickMode::Shuffle => "audio.pick_shuffle",
        PickMode::Sequence => "audio.pick_sequence",
    })
}

fn steal_name(steal: StealMode) -> String {
    t(match steal {
        StealMode::Oldest => "audio.steal_oldest",
        StealMode::Quietest => "audio.steal_quietest",
        StealMode::None => "audio.steal_none",
    })
}

pub fn rolloff_name(rolloff: Rolloff) -> String {
    t(match rolloff {
        Rolloff::Inverse => "audio.rolloff_inverse",
        Rolloff::Linear => "audio.rolloff_linear",
        Rolloff::Logarithmic => "audio.rolloff_logarithmic",
        Rolloff::Custom => "audio.rolloff_custom",
    })
}
