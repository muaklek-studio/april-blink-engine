//! The project's files: browse folders, open scenes, drag models and scenes into the
//! viewport or the scene tree. The game's scripts appear as a **Scripts** folder: create one
//! (Create → Script), double-click to open it in a code editor, drag it onto a node.

use super::ROW_SELECTED;
use crate::git::{FileState, GitClient};
use crate::i18n::{t, tf};
use crate::project::{AssetEntry, AssetKind};
use crate::scripts::{self, ScriptDrag, ScriptFile};
use crate::state::Editor;
use crate::theme;
use crate::viewport::AssetDrag;
use ab_project::script_templates::SCRIPT_TEMPLATES;
use eframe::egui::{self, Key, RichText, Sense};
use egui_phosphor::regular as icon;
use std::path::PathBuf;

/// The folder the panel shows for the game's scripts (they live in `src/scripts`, outside
/// the asset folder).
const SCRIPTS: &str = "#scripts";

/// Rust's orange, for script tiles.
const SCRIPT_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 140, 70);

#[derive(Default)]
pub struct AssetsPanel {
    /// Current folder, relative to the project ("" = root, [`SCRIPTS`] = the scripts).
    folder: String,
    filter: String,
    selected: Option<String>,
    create: Option<Create>,
    /// Script file waiting for the delete confirmation.
    delete: Option<PathBuf>,
    /// Previews of images, models, scenes and materials (made on first show).
    thumbnails: Option<crate::thumbnails::Thumbnails>,
    /// The window a double-click opens.
    pub viewer: crate::asset_viewer::AssetViewer,
}

/// The Create dialog.
struct Create {
    kind: CreateKind,
    name: String,
    /// Starting code: an index in `SCRIPT_TEMPLATES`, or the empty script.
    template: Option<usize>,
    open_after: bool,
    error: Option<String>,
    /// Focus the name field on the first frame.
    focus: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CreateKind {
    Script,
    Folder,
    Material,
}

enum Action {
    View(String, AssetKind),
    Enter(String),
    Open(String),
    Instance(String),
    OpenScript(PathBuf, usize),
    SystemOpen(PathBuf),
}

impl AssetsPanel {
    /// Starts in `folder`, with `view` open in the viewer.
    pub fn starting_at(
        folder: Option<&str>,
        view: Option<&str>,
        project: &std::path::Path,
    ) -> Self {
        let mut panel = Self {
            folder: folder.unwrap_or_default().trim_matches('/').to_owned(),
            ..Self::default()
        };
        if let Some(path) = view {
            let kind = AssetKind::of(std::path::Path::new(path));
            let _ = panel.viewer.open(project, path, kind);
        }
        panel
    }

    /// Goes to `path`'s folder with `path` selected.
    pub fn reveal(&mut self, path: &str) {
        self.folder = parent(path).to_owned();
        self.filter.clear();
        self.selected = Some(path.to_owned());
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        git: Option<&GitClient>,
        render_state: Option<&eframe::egui_wgpu::RenderState>,
    ) {
        let thumbnails = self.thumbnails.get_or_insert_with(|| {
            let gpu = render_state.map(|state| {
                ab_engine::rendering::GpuContext::from_shared(
                    state.adapter.clone(),
                    state.device.clone(),
                    state.queue.clone(),
                )
            });
            crate::thumbnails::Thumbnails::new(editor.project(), gpu)
        });
        if thumbnails.update(ui.ctx()) {
            ui.ctx().request_repaint();
        }
        if let Some(scripts) = &mut editor.scripts {
            scripts.refresh();
        } else if self.folder == SCRIPTS {
            self.folder.clear();
        }
        // Breadcrumbs, create, search, refresh.
        ui.horizontal(|ui| {
            if ui
                .button(icon::ARROW_CLOCKWISE)
                .on_hover_text(t("assets.rescan_hint"))
                .clicked()
            {
                editor.assets.request_rescan();
                if let Some(scripts) = &mut editor.scripts {
                    scripts.rescan();
                }
            }
            if ui
                .add_enabled(!self.folder.is_empty(), egui::Button::new(icon::ARROW_UP))
                .on_hover_text(t("assets.parent"))
                .clicked()
            {
                self.folder = if self.folder == SCRIPTS {
                    String::new()
                } else {
                    parent(&self.folder).to_owned()
                };
            }
            self.create_menu(ui, editor);
            ui.spacing_mut().item_spacing.x = 2.0;
            let root = editor
                .project()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".into());
            if ui
                .selectable_label(self.folder.is_empty(), format!("{}  {root}", icon::HOUSE))
                .clicked()
            {
                self.folder.clear();
            }
            if self.folder == SCRIPTS {
                ui.label(RichText::new(icon::CARET_RIGHT).color(theme::TEXT_DIM));
                let _ = ui.selectable_label(true, t("assets.scripts"));
            } else {
                let mut prefix = String::new();
                let parts: Vec<String> = self
                    .folder
                    .split('/')
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect();
                for part in parts {
                    ui.label(RichText::new(icon::CARET_RIGHT).color(theme::TEXT_DIM));
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(&part);
                    if ui.selectable_label(prefix == self.folder, part).clicked() {
                        self.folder = prefix.clone();
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(format!(
                            "{}  {}",
                            icon::MAGNIFYING_GLASS,
                            t("assets.search")
                        ))
                        .desired_width(220.0),
                );
            });
        });
        if let Some(warning) = editor.scripts.as_ref().and_then(|s| s.warning.clone()) {
            ui.label(RichText::new(format!("{}  {warning}", icon::WARNING)).color(theme::WARN));
        }
        ui.separator();

        let filter = self.filter.trim().to_lowercase();
        let in_scripts = self.folder == SCRIPTS && filter.is_empty();
        let entries: Vec<AssetEntry> = if in_scripts {
            Vec::new()
        } else {
            editor
                .assets
                .entries
                .iter()
                .filter(|e| {
                    if filter.is_empty() {
                        parent(&e.path) == self.folder
                    } else {
                        e.kind != AssetKind::Folder && e.path.to_lowercase().contains(&filter)
                    }
                })
                .cloned()
                .collect()
        };
        let script_files: Vec<ScriptFile> = match &editor.scripts {
            Some(scripts) if in_scripts => scripts.files.clone(),
            Some(scripts) if !filter.is_empty() => scripts
                .files
                .iter()
                .filter(|f| {
                    f.module.contains(&filter)
                        || f.types
                            .iter()
                            .any(|(t, _)| t.to_lowercase().contains(&filter))
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        let show_scripts_folder =
            editor.scripts.is_some() && self.folder.is_empty() && filter.is_empty();

        let mut action = None;
        // Right-click on the empty space between and below the tiles: the folder's menu.
        let background = ui.interact(
            ui.available_rect_before_wrap(),
            ui.id().with("assets_background"),
            Sense::click(),
        );
        if background.clicked() {
            self.selected = None;
        }
        background.context_menu(|ui| {
            ui.set_min_width(200.0);
            let here = if in_scripts {
                "src/scripts".to_owned()
            } else if self.folder.is_empty() {
                "assets".to_owned()
            } else {
                self.folder.clone()
            };
            ui.label(
                RichText::new(format!("{}  {here}", icon::FOLDER_OPEN)).color(theme::TEXT_DIM),
            );
            ui.separator();
            ui.menu_button(format!("{}  {}", icon::PLUS, t("menu.create")), |ui| {
                self.create_items(ui, editor);
            });
            if ui
                .button(format!("{}  {}", icon::ARROW_CLOCKWISE, t("assets.rescan")))
                .clicked()
            {
                editor.assets.request_rescan();
                ui.close();
            }
            if !self.folder.is_empty()
                && ui
                    .button(format!("{}  {}", icon::ARROW_UP, t("assets.parent")))
                    .clicked()
            {
                self.folder = if in_scripts {
                    String::new()
                } else {
                    parent(&self.folder).to_owned()
                };
                ui.close();
            }
            ui.separator();
            let dir = if in_scripts {
                editor.scripts.as_ref().map(|s| s.dir())
            } else {
                Some(editor.project().join(&self.folder))
            };
            if let Some(dir) = dir {
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::ARROW_SQUARE_OUT,
                        t("assets.reveal")
                    ))
                    .clicked()
                {
                    action = Some(Action::SystemOpen(dir.clone()));
                    ui.close();
                }
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::CLIPBOARD_TEXT,
                        t("assets.copy_folder_path")
                    ))
                    .clicked()
                {
                    ui.ctx().copy_text(dir.display().to_string());
                    ui.close();
                }
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if entries.is_empty() && script_files.is_empty() && !show_scripts_folder {
                    if in_scripts {
                        super::empty_hint(ui, icon::FILE_RS, &t("assets.no_scripts"));
                    } else {
                        super::empty_hint(ui, icon::FOLDER_OPEN, &t("assets.empty"));
                    }
                    return;
                }
                // Tiles that wrap with the panel width.
                let tile = egui::vec2(96.0, 104.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    if show_scripts_folder {
                        if let Some(a) = self.scripts_folder_tile(ui, editor, tile) {
                            action = Some(a);
                        }
                    }
                    for file in &script_files {
                        let change = git.and_then(|g| g.file_state(&file.path));
                        if let Some(a) = self.script_tile(ui, file, tile, change) {
                            action = Some(a);
                        }
                    }
                    for entry in &entries {
                        let change =
                            git.and_then(|g| g.file_state(&editor.project().join(&entry.path)));
                        if let Some(a) = self.tile(ui, entry, tile, !filter.is_empty(), change) {
                            action = Some(a);
                        }
                    }
                });
            });

        match action {
            Some(Action::View(path, kind)) => {
                if !self.viewer.open(editor.project(), &path, kind) {
                    editor.fail(t("assets.viewers_pinned"));
                }
            }
            Some(Action::Enter(folder)) => {
                self.folder = folder;
                self.filter.clear();
            }
            Some(Action::Open(path)) => crate::app::request_open(ui.ctx(), path),
            Some(Action::Instance(path)) => crate::app::instance_asset(editor, &path, None),
            Some(Action::OpenScript(path, line)) => open_script(editor, &path, line),
            Some(Action::SystemOpen(path)) => {
                if let Err(err) = scripts::open_with_system(&path) {
                    editor.fail(format!("{err:#}"));
                }
            }
            None => {}
        }
        self.create_dialog(ui.ctx(), editor);
        self.delete_dialog(ui.ctx(), editor);
    }

    fn create_menu(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        ui.menu_button(format!("{}  {}", icon::PLUS, t("menu.create")), |ui| {
            self.create_items(ui, editor);
        });
    }

    /// The Create entries (the Create button and the empty space's menu).
    fn create_items(&mut self, ui: &mut egui::Ui, editor: &Editor) {
        {
            ui.set_min_width(180.0);
            let can_script = editor.scripts.is_some();
            let script = ui
                .add_enabled(
                    can_script,
                    egui::Button::new(format!("{}  {}", icon::FILE_RS, t("assets.script_item"))),
                )
                .on_disabled_hover_text(t("assets.no_crate_hint"));
            if script.clicked() {
                self.create = Some(Create {
                    kind: CreateKind::Script,
                    name: "NewScript".into(),
                    template: None,
                    open_after: true,
                    error: None,
                    focus: true,
                });
                ui.close();
            }
            let folder = ui.add_enabled(
                self.folder != SCRIPTS,
                egui::Button::new(format!(
                    "{}  {}",
                    icon::FOLDER_PLUS,
                    t("assets.folder_item")
                )),
            );
            if folder.clicked() {
                self.create = Some(Create {
                    kind: CreateKind::Folder,
                    name: "New Folder".into(),
                    template: None,
                    open_after: false,
                    error: None,
                    focus: true,
                });
                ui.close();
            }
            let material = ui.add_enabled(
                self.folder != SCRIPTS,
                egui::Button::new(format!(
                    "{}  {}",
                    icon::DROP_HALF,
                    t("assets.material_item")
                )),
            );
            if material.clicked() {
                self.create = Some(Create {
                    kind: CreateKind::Material,
                    name: "new_material".into(),
                    template: None,
                    open_after: false,
                    error: None,
                    focus: true,
                });
                ui.close();
            }
        }
    }

    fn create_dialog(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        let Some(create) = &mut self.create else {
            return;
        };
        let mut close = false;
        let mut done: Option<(CreateKind, String, Option<usize>)> = None;
        let modal = egui::Modal::new(egui::Id::new("asset_create")).show(ctx, |ui| {
            ui.set_width(420.0);
            let (glyph, title) = match create.kind {
                CreateKind::Script => (icon::FILE_RS, t("assets.new_script")),
                CreateKind::Folder => (icon::FOLDER_PLUS, t("assets.new_folder")),
                CreateKind::Material => (icon::DROP_HALF, t("assets.new_material")),
            };
            ui.heading(format!("{glyph}  {title}"));
            ui.add_space(4.0);
            let edit =
                ui.add(egui::TextEdit::singleline(&mut create.name).desired_width(f32::INFINITY));
            if std::mem::take(&mut create.focus) {
                edit.request_focus();
            }
            let preview = match create.kind {
                CreateKind::Script => scripts::type_name(&create.name).map(|name| {
                    tf(
                        "assets.script_preview",
                        &[
                            ("name", &name),
                            (
                                "path",
                                &format!("src/scripts/{}.rs", scripts::module_name(&name)),
                            ),
                        ],
                    )
                }),
                CreateKind::Folder => folder_name(&create.name).map(|name| {
                    tf(
                        "assets.folder_preview",
                        &[("path", &join(&self.folder, &name))],
                    )
                }),
                CreateKind::Material => folder_name(&create.name).map(|name| {
                    let file = format!("{name}.{}", ab_engine::scene_file::MATERIAL_EXTENSION);
                    tf(
                        "assets.material_preview",
                        &[("path", &join(&self.folder, &file))],
                    )
                }),
            };
            match (&preview, &create.error) {
                (_, Some(error)) => {
                    ui.label(RichText::new(error).color(theme::ERROR));
                }
                (Ok(text), None) => {
                    ui.label(RichText::new(text).color(theme::TEXT_DIM));
                }
                (Err(error), None) => {
                    ui.label(RichText::new(error).color(theme::WARN));
                }
            }
            if create.kind == CreateKind::Script {
                template_picker(ui, create);
                ui.checkbox(&mut create.open_after, t("assets.open_after"));
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let valid = preview.is_ok();
                let button = ui.add_enabled(
                    valid,
                    egui::Button::new(RichText::new(t("menu.create")).strong())
                        .fill(theme::ACCENT_DIM),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if valid && (button.clicked() || enter) {
                    done = Some((create.kind, create.name.clone(), create.template));
                }
                if ui.button(t("dialog.cancel")).clicked() {
                    close = true;
                }
            });
            if edit.changed() {
                create.error = None;
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some((kind, name, template)) = done {
            let open_after = create.open_after;
            match self.create_asset(editor, kind, &name, template, open_after) {
                Ok(()) => close = true,
                Err(err) => {
                    if let Some(create) = &mut self.create {
                        create.error = Some(err);
                    }
                }
            }
        }
        if close {
            self.create = None;
        }
    }

    fn create_asset(
        &mut self,
        editor: &mut Editor,
        kind: CreateKind,
        name: &str,
        template: Option<usize>,
        open_after: bool,
    ) -> Result<(), String> {
        match kind {
            CreateKind::Script => {
                let template = template.and_then(|i| SCRIPT_TEMPLATES.get(i));
                let taken = editor.registered_script_names();
                let scripts = editor
                    .scripts
                    .as_mut()
                    .ok_or_else(|| t("assets.no_crate"))?;
                let (name, path) = scripts
                    .create_from(name, &taken, template)
                    .map_err(|e| format!("{e:#}"))?;
                self.folder = SCRIPTS.into();
                self.filter.clear();
                self.selected = Some(path.to_string_lossy().into_owned());
                let added = match template {
                    Some(t) => editor
                        .add_template_actions(t)
                        .map_err(|e| format!("{e:#}"))?,
                    None => Vec::new(),
                };
                let attach =
                    template.map_or_else(|| t("assets.a_node"), |t| t.attach_to.to_owned());
                if added.is_empty() {
                    editor.info(tf(
                        "assets.script_created",
                        &[("name", &name), ("node", &attach)],
                    ));
                } else {
                    editor.info(tf(
                        "assets.script_created_actions",
                        &[
                            ("name", &name),
                            ("node", &attach),
                            ("actions", &added.join(", ")),
                        ],
                    ));
                }
                if open_after {
                    let line = editor
                        .scripts
                        .as_ref()
                        .and_then(|s| s.find(&name))
                        .map_or(1, |(_, line)| line);
                    open_script(editor, &path, line);
                }
                Ok(())
            }
            CreateKind::Material => {
                let name = folder_name(name)?;
                let file = format!("{name}.{}", ab_engine::scene_file::MATERIAL_EXTENSION);
                let relative = join(&self.folder, &file);
                let path = editor.project().join(&relative);
                if path.exists() {
                    return Err(tf("assets.exists", &[("path", &relative)]));
                }
                let material = ab_engine::scene_file::MaterialDesc {
                    color: Some(ab_engine::scene_file::ColorDesc::Hex("#c8ccd4".into())),
                    roughness: Some(0.6),
                    metallic: Some(0.0),
                    ..Default::default()
                };
                editor
                    .write_project_file(&path, material.to_toml())
                    .map_err(|e| e.to_string())?;
                editor.assets.request_rescan();
                self.selected = Some(relative.clone());
                editor.info(tf("assets.material_created", &[("path", &relative)]));
                Ok(())
            }
            CreateKind::Folder => {
                let name = folder_name(name)?;
                let relative = join(&self.folder, &name);
                let path = editor.project().join(&relative);
                if path.exists() {
                    return Err(tf("assets.exists", &[("path", &relative)]));
                }
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
                editor.assets.request_rescan();
                self.folder = relative;
                Ok(())
            }
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        let Some(path) = self.delete.clone() else {
            return;
        };
        let mut close = false;
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let modal = egui::Modal::new(egui::Id::new("script_delete")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(format!("{}  {}", icon::TRASH, t("assets.delete_script")));
            ui.label(tf(
                "assets.delete_body",
                &[("path", &format!("src/scripts/{file}"))],
            ));
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(
                        RichText::new(t("command.delete")).color(theme::ERROR),
                    ))
                    .clicked()
                {
                    let result = editor.scripts.as_mut().map(|s| s.delete(&path));
                    match result {
                        Some(Ok(())) => editor.info(tf("assets.deleted", &[("file", &file)])),
                        Some(Err(err)) => editor.fail(format!("{err:#}")),
                        None => {}
                    }
                    close = true;
                }
                if ui.button(t("dialog.cancel")).clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.delete = None;
        }
    }

    fn scripts_folder_tile(
        &mut self,
        ui: &mut egui::Ui,
        editor: &Editor,
        size: egui::Vec2,
    ) -> Option<Action> {
        let count = editor.scripts.as_ref().map_or(0, |s| s.files.len());
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let fill = if response.hovered() {
            theme::BG_3
        } else {
            theme::BG_2
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        tile_icon(ui, rect, icon::FOLDER_NOTCH_OPEN, SCRIPT_COLOR);
        tile_label(ui, rect, &tf("assets.scripts_count", &[("count", &count)]));
        let response = response.on_hover_text(tf("assets.scripts_hint", &[("count", &count)]));
        let mut action = None;
        if response.double_clicked() {
            action = Some(Action::Enter(SCRIPTS.into()));
        }
        response.context_menu(|ui| {
            if ui
                .button(format!("{}  {}", icon::FOLDER_OPEN, t("assets.open")))
                .clicked()
            {
                action = Some(Action::Enter(SCRIPTS.into()));
                ui.close();
            }
            if let Some(scripts) = &editor.scripts {
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::ARROW_SQUARE_OUT,
                        t("assets.reveal")
                    ))
                    .clicked()
                {
                    action = Some(Action::SystemOpen(scripts.dir()));
                    ui.close();
                }
            }
        });
        action
    }

    fn script_tile(
        &mut self,
        ui: &mut egui::Ui,
        file: &ScriptFile,
        size: egui::Vec2,
        change: Option<FileState>,
    ) -> Option<Action> {
        let key = file.path.to_string_lossy().into_owned();
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let selected = self.selected.as_deref() == Some(key.as_str());
        let fill = if selected {
            ROW_SELECTED
        } else if response.hovered() {
            theme::BG_3
        } else {
            theme::BG_2
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        let has_script = !file.types.is_empty();
        tile_icon(
            ui,
            rect,
            icon::FILE_RS,
            if has_script {
                SCRIPT_COLOR
            } else {
                theme::TEXT_DIM
            },
        );
        let label = file
            .main_type()
            .map_or_else(|| file.file_name(), str::to_owned);
        tile_label(ui, rect, &label);
        if let Some(change) = change {
            ui.painter().text(
                rect.right_top() + egui::vec2(-6.0, 4.0),
                egui::Align2::RIGHT_TOP,
                change.letter(),
                egui::FontId::monospace(11.0),
                crate::panels::git::state_color(change),
            );
        }
        let types: Vec<&str> = file.types.iter().map(|(t, _)| t.as_str()).collect();
        let hover = if has_script {
            tf(
                "assets.script_hint",
                &[
                    ("types", &types.join(", ")),
                    ("path", &format!("src/scripts/{}", file.file_name())),
                ],
            )
        } else {
            tf(
                "assets.no_script_type",
                &[("path", &format!("src/scripts/{}", file.file_name()))],
            )
        };
        let response = response.on_hover_text(hover);
        if let Some(name) = file.main_type() {
            response.dnd_set_drag_payload(ScriptDrag(name.to_owned()));
        }
        if response.clicked() {
            self.selected = Some(key);
        }
        let line = file.types.first().map_or(1, |(_, line)| *line);
        let mut action = None;
        if response.double_clicked() {
            action = Some(Action::OpenScript(file.path.clone(), line));
        }
        response.context_menu(|ui| {
            let editor_name = scripts::code_editor()
                .map_or_else(|| t("assets.default_app"), |e| e.name.to_owned());
            if ui
                .button(format!(
                    "{}  {}",
                    icon::CODE,
                    tf("assets.open_in", &[("app", &editor_name)])
                ))
                .clicked()
            {
                action = Some(Action::OpenScript(file.path.clone(), line));
                ui.close();
            }
            if ui
                .button(format!(
                    "{}  {}",
                    icon::ARROW_SQUARE_OUT,
                    t("assets.reveal")
                ))
                .clicked()
            {
                action = Some(Action::SystemOpen(
                    file.path.parent().unwrap_or(&file.path).to_path_buf(),
                ));
                ui.close();
            }
            if let Some(name) = file.main_type() {
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::CLIPBOARD_TEXT,
                        t("assets.copy_script_name")
                    ))
                    .clicked()
                {
                    ui.ctx().copy_text(name.to_owned());
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .button(
                    RichText::new(format!("{}  {}", icon::TRASH, t("assets.delete_item")))
                        .color(theme::ERROR),
                )
                .clicked()
            {
                self.delete = Some(file.path.clone());
                ui.close();
            }
        });
        action
    }

    fn tile(
        &mut self,
        ui: &mut egui::Ui,
        entry: &AssetEntry,
        size: egui::Vec2,
        show_path: bool,
        change: Option<FileState>,
    ) -> Option<Action> {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let selected = self.selected.as_deref() == Some(entry.path.as_str());
        let fill = if selected {
            ROW_SELECTED
        } else if response.hovered() {
            theme::BG_3
        } else {
            theme::BG_2
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        let color = kind_color(entry.kind);
        let picture = tile_picture(rect);
        let thumbnail = self
            .thumbnails
            .as_mut()
            .and_then(|t| t.get(ui.ctx(), &entry.path, entry.kind))
            .cloned();
        match thumbnail.as_ref().and_then(|t| t.texture.as_ref()) {
            Some(texture) => {
                let [w, h] = texture.size().map(|v| v as f32);
                let fit = (picture.width() / w).min(picture.height() / h);
                let shown = egui::Rect::from_center_size(picture.center(), egui::vec2(w, h) * fit);
                ui.painter().image(
                    texture.id(),
                    shown,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => tile_icon(ui, rect, entry.kind.icon(), color),
        }
        // What kind of file it is, small, over the picture.
        if thumbnail.as_ref().is_some_and(|t| t.texture.is_some()) {
            let badge = if thumbnail.as_ref().is_some_and(|t| t.normal_map) {
                "N"
            } else {
                entry.kind.icon()
            };
            let at = picture.left_top() + egui::vec2(2.0, 2.0);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(at, egui::vec2(16.0, 15.0)),
                3.0,
                theme::BG_0.gamma_multiply(0.85),
            );
            ui.painter().text(
                at + egui::vec2(8.0, 7.5),
                egui::Align2::CENTER_CENTER,
                badge,
                egui::FontId::proportional(10.5),
                color,
            );
        }
        let name = if show_path { &entry.path } else { &entry.name };
        tile_label(ui, rect, name);
        if let Some(change) = change {
            ui.painter().text(
                rect.right_top() + egui::vec2(-6.0, 4.0),
                egui::Align2::RIGHT_TOP,
                change.letter(),
                egui::FontId::monospace(11.0),
                crate::panels::git::state_color(change),
            );
        }
        let response = match change {
            Some(change) => {
                response.on_hover_text(format!("{} ({})", entry.path, change.describe()))
            }
            None => response.on_hover_text(&entry.path),
        };

        let draggable = matches!(entry.kind, AssetKind::Model | AssetKind::Scene);
        if draggable {
            response.dnd_set_drag_payload(AssetDrag(entry.path.clone()));
        }
        if response.clicked() {
            self.selected = Some(entry.path.clone());
        }
        let mut action = None;
        if response.double_clicked() {
            action = match entry.kind {
                AssetKind::Folder => Some(Action::Enter(entry.path.clone())),
                AssetKind::Scene => Some(Action::Open(entry.path.clone())),
                kind if crate::asset_viewer::can_view(kind) => {
                    Some(Action::View(entry.path.clone(), kind))
                }
                _ => None,
            };
        }
        response.context_menu(|ui| {
            if entry.kind == AssetKind::Scene
                && ui
                    .button(format!("{}  {}", icon::FOLDER_OPEN, t("viewer.open_scene")))
                    .clicked()
            {
                action = Some(Action::Open(entry.path.clone()));
                ui.close();
            }
            if crate::asset_viewer::can_view(entry.kind)
                && ui
                    .button(format!("{}  {}", icon::EYE, t("assets.view")))
                    .clicked()
            {
                action = Some(Action::View(entry.path.clone(), entry.kind));
                ui.close();
            }
            if draggable
                && ui
                    .button(format!(
                        "{}  {}",
                        icon::PLUS_CIRCLE,
                        t("viewer.add_to_scene")
                    ))
                    .clicked()
            {
                action = Some(Action::Instance(entry.path.clone()));
                ui.close();
            }
            if ui
                .button(format!("{}  {}", icon::CLIPBOARD_TEXT, t("menu.copy_path")))
                .clicked()
            {
                ui.ctx().copy_text(entry.path.clone());
                ui.close();
            }
        });
        action
    }
}

/// Where a tile's picture goes: a square at its top.
fn tile_picture(tile: egui::Rect) -> egui::Rect {
    let side = tile.width() - 22.0;
    egui::Rect::from_min_size(
        egui::pos2(tile.center().x - side / 2.0, tile.top() + 6.0),
        egui::Vec2::splat(side),
    )
}

/// A tile's icon, for files and folders without a thumbnail.
fn tile_icon(ui: &egui::Ui, tile: egui::Rect, glyph: &str, color: egui::Color32) {
    let picture = tile_picture(tile);
    ui.painter().text(
        picture.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(picture.height() * 0.5),
        color,
    );
}

/// A tile's name under its picture: two lines at most, cut with "…".
fn tile_label(ui: &egui::Ui, tile: egui::Rect, text: &str) {
    let picture = tile_picture(tile);
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(11.5),
        ui.visuals().text_color(),
        tile.width() - 8.0,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    ui.painter().with_clip_rect(tile.shrink(2.0)).galley(
        egui::pos2(tile.center().x, picture.bottom() + 3.0),
        galley,
        ui.visuals().text_color(),
    );
}

/// Opens a script in the code editor, reporting which one in the status bar.
pub fn open_script(editor: &mut Editor, path: &std::path::Path, line: usize) {
    let folder = editor
        .scripts
        .as_ref()
        .map(|s| s.crate_root.clone())
        .unwrap_or_else(|| path.parent().unwrap_or(path).to_path_buf());
    match scripts::open_script(path, line, &folder) {
        Ok(with) => editor.info(tf(
            "assets.opened",
            &[
                (
                    "file",
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                ),
                ("app", &with),
            ],
        )),
        Err(err) => editor.fail(tf("assets.open_failed", &[("error", &format!("{err:#}"))])),
    }
}

/// A folder name the user typed, checked.
fn folder_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    if name.is_empty() {
        return Err(t("assets.name_empty"));
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || name.starts_with('.') {
        return Err(t("assets.name_invalid"));
    }
    Ok(name.to_owned())
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn kind_color(kind: AssetKind) -> egui::Color32 {
    match kind {
        AssetKind::Folder => theme::WARN,
        AssetKind::Scene => theme::ACCENT,
        AssetKind::Model => egui::Color32::from_rgb(120, 190, 255),
        AssetKind::Material => egui::Color32::from_rgb(110, 220, 190),
        AssetKind::Image => egui::Color32::from_rgb(180, 140, 255),
        AssetKind::Audio => egui::Color32::from_rgb(240, 130, 180),
        AssetKind::Font => egui::Color32::from_rgb(200, 200, 200),
        AssetKind::Config => theme::TEXT_DIM,
        AssetKind::Other => theme::TEXT_DIM,
    }
}

/// The starting code of a new script: empty, or one of the templates, by category.
fn template_picker(ui: &mut egui::Ui, create: &mut Create) {
    ui.add_space(4.0);
    let chosen = create.template.and_then(|i| SCRIPT_TEMPLATES.get(i));
    let empty = t("assets.empty_script");
    egui::ComboBox::from_id_salt("script_template")
        .selected_text(chosen.map_or(empty.as_str(), |t| t.title))
        .width(ui.available_width())
        .height(420.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut create.template, None, empty.as_str());
            let mut category = "";
            for (i, template) in SCRIPT_TEMPLATES.iter().enumerate() {
                if template.category != category {
                    category = template.category;
                    ui.label(
                        RichText::new(category)
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                }
                ui.selectable_value(&mut create.template, Some(i), template.title)
                    .on_hover_text(template.description);
            }
        });
    if let Some(template) = create.template.and_then(|i| SCRIPT_TEMPLATES.get(i)) {
        // Name the script after the template until the user names it.
        if create.name == "NewScript" || SCRIPT_TEMPLATES.iter().any(|t| t.type_name == create.name)
        {
            create.name = template.type_name.to_owned();
        }
        ui.label(RichText::new(template.description).small());
        ui.label(
            RichText::new(tf("assets.attach_to", &[("node", &template.attach_to)]))
                .small()
                .color(theme::TEXT_DIM),
        );
        let actions: Vec<&str> = template.actions().map(|(name, _)| *name).collect();
        if !actions.is_empty() {
            ui.label(
                RichText::new(tf(
                    "assets.template_actions",
                    &[("actions", &actions.join(", "))],
                ))
                .small()
                .color(theme::TEXT_DIM),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        panel: &mut AssetsPanel,
        editor: &mut Editor,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 500.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| panel.show(ui, editor, None, None));
        output.textures_delta.clear();
    }

    #[test]
    fn right_click_on_empty_space_opens_the_folder_menu() {
        let project =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let mut editor = Editor::new(project, None, ab_engine::audio::AudioOutput::Silent);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut panel = AssetsPanel::default();
        frame(&ctx, &mut panel, &mut editor, Vec::new());
        // Far below the one row of folders.
        let at = egui::pos2(450.0, 450.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
            vec![],
        ] {
            frame(&ctx, &mut panel, &mut editor, events);
        }
        assert!(egui::Popup::is_any_open(&ctx), "the folder's menu is open");
    }

    #[test]
    fn parent_folders() {
        assert_eq!(super::parent("models/crate.glb"), "models");
        assert_eq!(super::parent("a/b/c.toml"), "a/b");
        assert_eq!(super::parent("input.toml"), "");
    }

    #[test]
    fn folder_names() {
        assert_eq!(super::folder_name(" Enemies ").unwrap(), "Enemies");
        assert!(super::folder_name("").is_err());
        assert!(super::folder_name("a/b").is_err());
        assert!(super::folder_name(".hidden").is_err());
        assert_eq!(super::join("", "x"), "x");
        assert_eq!(super::join("models", "x"), "models/x");
    }
}
