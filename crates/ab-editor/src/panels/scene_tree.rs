//! The scene hierarchy: search, selection, inline rename, drag-and-drop reparenting and a
//! context menu.

use super::ROW_SELECTED;
use crate::commands::{self, Command};
use crate::features::{self, Feature};
use crate::scripts::ScriptDrag;
use crate::state::Editor;
use crate::theme;
use crate::viewport::AssetDrag;
use ab_engine::core::Vec3;
use ab_engine::scene::NodeHandle;
use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
use std::collections::HashSet;

const ROW_HEIGHT: f32 = 20.0;

/// A node dragged inside the tree.
#[derive(Debug, Clone, Copy)]
struct NodeDrag(NodeHandle);

/// What a drop on a row means.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DropAt {
    Before,
    Into,
    After,
}

#[derive(Default)]
pub struct SceneTreePanel {
    filter: String,
    /// Folded nodes, by path (handles change when the scene is rebuilt).
    collapsed: HashSet<String>,
    /// Prefab instances unfolded (they start folded), by path.
    expanded: HashSet<String>,
    renaming: Option<(NodeHandle, String)>,
    /// Scroll the selection into view once (it changed elsewhere).
    reveal: Option<NodeHandle>,
    last_selection: Option<NodeHandle>,
    /// Double-clicked: the viewport should frame the selection.
    pub frame_request: bool,
    /// The rows drawn this frame, top to bottom (Shift+click selects a range of them).
    rows: Vec<NodeHandle>,
    /// Show only this node and what is under it (a tool window's subject).
    pub root: Option<NodeHandle>,
}

/// Deferred edits, applied after drawing (the tree is borrowed while drawing).
enum Action {
    /// A click (with the modifiers of this frame: Ctrl toggles, Shift selects a range).
    Select(NodeHandle),
    /// A right-click, before the context menu acts.
    SelectForMenu(NodeHandle),
    Rename(NodeHandle, String),
    Delete(NodeHandle),
    Duplicate(NodeHandle),
    Move(NodeHandle, isize),
    Drop(NodeHandle, NodeHandle, DropAt),
    Instance(String, NodeHandle),
    AttachScript(NodeHandle, String),
    ToggleVisible(NodeHandle),
    /// Paste the clipboard under this node.
    PasteInto(NodeHandle),
    /// Save as a prefab and replace with an instance of it.
    MakePrefab(NodeHandle),
    /// Turn a generator's output into ordinary nodes.
    BakeGenerator(NodeHandle),
    /// Open the node in its own editor (a UI node, an AnimationPlayer, an effect).
    Open(NodeHandle),
}

impl SceneTreePanel {
    pub fn start_rename(&mut self, editor: &Editor) {
        if let Some(node) = editor.selection.filter(|&n| editor.is_editable(n)) {
            let name = editor
                .tree()
                .name(node)
                .map(|n| n.to_owned())
                .unwrap_or_default();
            self.renaming = Some((node, name));
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, editor: &mut Editor, focus: Vec3) {
        if editor.selection != self.last_selection {
            self.reveal = editor.selection;
            self.last_selection = editor.selection;
            // Unfold the ancestors of a selection made elsewhere (viewport picking).
            if let Some(node) = editor.selection {
                let mut current = editor.tree().parent(node);
                while let Some(parent) = current {
                    if let Some(path) = editor.relative_path(parent) {
                        self.collapsed.remove(&path);
                    }
                    if parent == editor.scene_root {
                        break;
                    }
                    current = editor.tree().parent(parent);
                }
            }
        }

        ui.horizontal(|ui| {
            ui.add_enabled_ui(!editor.is_playing(), |ui| {
                ui.menu_button(RichText::new(icon::PLUS).strong(), |ui| {
                    crate::create::menu(ui, editor, focus);
                })
                .response
                .on_hover_text(crate::i18n::t("scene_tree.add_hint"));
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(format!(
                        "{}  {}",
                        icon::MAGNIFYING_GLASS,
                        crate::i18n::t("scene_tree.filter")
                    ))
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add_space(2.0);

        let mut actions = Vec::new();
        let previous_rows = std::mem::take(&mut self.rows);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let filter = self.filter.trim().to_lowercase();
                if filter.is_empty() {
                    let root = self
                        .root
                        .filter(|&r| editor.tree().is_valid(r))
                        .unwrap_or(editor.scene_root);
                    self.row(ui, editor, root, 0, &mut actions);
                } else {
                    self.filtered(ui, editor, &filter, &mut actions);
                }
                // Dropping below the rows moves to the end of the scene root.
                let rest = ui.allocate_response(
                    egui::vec2(ui.available_width(), ui.available_height().max(40.0)),
                    Sense::click(),
                );
                if let Some(drag) = rest.dnd_release_payload::<NodeDrag>() {
                    actions.push(Action::Drop(drag.0, editor.scene_root, DropAt::Into));
                }
                if let Some(path) = rest.dnd_release_payload::<AssetDrag>() {
                    actions.push(Action::Instance(path.0.clone(), editor.scene_root));
                }
                if rest.clicked() {
                    editor.select(None);
                }
                if rest.secondary_clicked() {
                    editor.select(None);
                }
                rest.context_menu(|ui| {
                    ui.set_min_width(200.0);
                    ui.add_enabled_ui(!editor.is_playing(), |ui| {
                        ui.menu_button(
                            format!("{}  {}", icon::PLUS, crate::i18n::t("menu.create")),
                            |ui| {
                                crate::create::menu(ui, editor, focus);
                            },
                        );
                    });
                    if ui
                        .add_enabled(
                            editor.can_paste(None),
                            egui::Button::new(format!(
                                "{}  {}",
                                icon::CLIPBOARD,
                                Command::Paste.label()
                            )),
                        )
                        .clicked()
                    {
                        actions.push(Action::PasteInto(editor.scene_root));
                        ui.close();
                    }
                    ui.separator();
                    commands::menu_item(ui, Command::SelectAll, !editor.is_playing());
                    commands::menu_item(ui, Command::SelectNone, !editor.selected.is_empty());
                });
            });
        self.reveal = None;

        let modifiers = ui.input(|i| i.modifiers);
        for action in actions {
            match action {
                Action::Select(node) => select_row(editor, node, modifiers, &previous_rows),
                Action::Open(node) => {
                    editor.select(Some(node));
                    editor.open_request = Some(node);
                }
                Action::SelectForMenu(node) => {
                    // Right-clicking inside a multi-selection keeps it.
                    if !editor.is_selected(node) {
                        editor.select(Some(node));
                    }
                }
                Action::Rename(node, name) => {
                    editor.rename(node, &name);
                    editor.end_gesture();
                }
                Action::Delete(node) if editor.is_selected(node) => editor.delete_selected(),
                Action::Delete(node) => editor.delete(node),
                Action::Duplicate(node) if editor.is_selected(node) => {
                    editor.duplicate_selected();
                }
                Action::Duplicate(node) => editor.duplicate(node),
                Action::Move(node, step) => editor.move_sibling(node, step),
                Action::Drop(node, target, at) => {
                    // Dragging a selected node moves the whole selection.
                    let nodes = if editor.is_selected(node) {
                        let mut roots = editor.selected_roots();
                        // In tree order, so they keep their order at the drop.
                        let order = editor.tree().subtree(editor.scene_root);
                        roots.sort_by_key(|n| order.iter().position(|o| o == n));
                        roots
                    } else {
                        vec![node]
                    };
                    drop_nodes(editor, &nodes, target, at);
                }
                // Inside an instance (a prefab's part) it would not be saved.
                Action::Instance(_, parent) if !editor.is_editable(parent) => {
                    editor.fail(crate::i18n::t("status.instance_contents"));
                }
                Action::Instance(path, parent) => {
                    editor.select(Some(parent));
                    crate::app::instance_asset(editor, &path, None);
                }
                Action::AttachScript(node, name) => editor.attach_script(node, &name),
                Action::PasteInto(node) => editor.paste(None, Some(node), None),
                Action::MakePrefab(node) => {
                    let path = crate::panels::scene_tree::prefab_path(editor, node);
                    match editor.create_prefab(node, &path) {
                        Ok(instance) => editor.select(Some(instance)),
                        Err(err) => editor.fail(err),
                    }
                }
                Action::BakeGenerator(node) => match editor.bake_generator(node) {
                    Ok(group) => editor.select(Some(group)),
                    Err(err) => editor.fail(err),
                },
                Action::ToggleVisible(node) => {
                    if let Some(old) = editor.describe_node(node) {
                        let mut new = old.clone();
                        new.visible = if old.visible == Some(false) {
                            None
                        } else {
                            Some(false)
                        };
                        editor.apply_desc(node, &old, new);
                        editor.end_gesture();
                    }
                }
            }
        }
        self.last_selection = editor.selection;
    }

    fn filtered(
        &mut self,
        ui: &mut egui::Ui,
        editor: &Editor,
        filter: &str,
        actions: &mut Vec<Action>,
    ) {
        let tree = editor.tree();
        let mut any = false;
        for node in tree.subtree(editor.scene_root) {
            if !editor.is_editable(node) {
                continue;
            }
            // Inside a UI node or an effect: found in that one's editor.
            if features::owner(editor, node).is_some_and(|o| o != node) {
                continue;
            }
            let name = tree.name(node).map(|n| n.to_owned()).unwrap_or_default();
            let class = tree.class_name(node).unwrap_or("Node");
            let script = tree.script_names(node).join(" ");
            let haystack = format!("{name} {class} {script}").to_lowercase();
            if !haystack.contains(filter) {
                continue;
            }
            any = true;
            self.rows.push(node);
            let selected = editor.is_selected(node);
            let path = editor.relative_path(node).unwrap_or_default();
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            paint_row_background(ui, rect, selected, response.hovered());
            let painter = ui.painter();
            painter.text(
                rect.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                theme::class_icon(class),
                egui::FontId::proportional(13.0),
                theme::class_color(class),
            );
            painter.text(
                rect.left_center() + egui::vec2(28.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &name,
                egui::FontId::proportional(13.0),
                ui.visuals().text_color(),
            );
            painter.text(
                rect.right_center() - egui::vec2(8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                path,
                egui::FontId::proportional(11.5),
                theme::TEXT_DIM,
            );
            if response.clicked() {
                actions.push(Action::Select(node));
            }
        }
        if !any {
            super::empty_hint(
                ui,
                icon::MAGNIFYING_GLASS,
                &crate::i18n::t("scene_tree.no_match"),
            );
        }
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        editor: &Editor,
        node: NodeHandle,
        depth: usize,
        actions: &mut Vec<Action>,
    ) {
        let tree = editor.tree();
        let Some(name) = tree.name(node).map(|n| n.to_owned()) else {
            return;
        };
        let class = tree.class_name(node).unwrap_or("Node");
        let path = editor.relative_path(node).unwrap_or_default();
        let instance = editor.is_instance(node);
        // A prefab's nodes can be changed (overrides): shown, folded at first. A model's
        // contents are not part of the scene: shown as a leaf.
        let prefab = instance
            && tree
                .children(node)
                .first()
                .is_some_and(|&c| editor.is_override_target(c));
        // A UI node's controls and an effect's layers are edited in their own window.
        let feature = Feature::of(editor, node);
        let owned = feature.is_some_and(Feature::owns_children);
        // A tool window's subject shows what is inside it, a model's bones too.
        let subject = self.root == Some(node);
        let children: Vec<NodeHandle> = if ((instance && !prefab) || owned) && !subject {
            Vec::new()
        } else {
            tree.children(node)
        };
        let open = if subject {
            true
        } else if prefab {
            self.expanded.contains(&path)
        } else {
            !self.collapsed.contains(&path)
        };
        // Something selected inside (from the viewport, say) shows on the node holding it.
        let selected = editor.is_selected(node)
            || (owned
                && editor
                    .selected
                    .iter()
                    .any(|&s| features::owner(editor, s) == Some(node)));
        let active = editor.selection == Some(node);
        self.rows.push(node);
        let visible = tree.is_visible(node);
        // The first script, and how many more.
        let scripts = tree.script_names(node);
        let script = scripts.first().map(|first| match scripts.len() {
            1 => first.clone(),
            n => format!("{first} +{}", n - 1),
        });

        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(200.0), ROW_HEIGHT),
            Sense::click_and_drag(),
        );
        if self.reveal == Some(node) {
            response.scroll_to_me(Some(egui::Align::Center));
        }
        paint_row_background(ui, rect, selected, response.hovered());
        if active && editor.selected.len() > 1 {
            // The active node of a multi-selection (shown in the inspector).
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                1.0,
                theme::ACCENT,
            );
        }

        // Drop target feedback.
        let mut drop_at = None;
        if let (Some(_), Some(pointer)) = (
            response.dnd_hover_payload::<NodeDrag>(),
            ui.input(|i| i.pointer.hover_pos()),
        ) {
            let t = (pointer.y - rect.top()) / rect.height();
            let at = if node == editor.scene_root {
                DropAt::Into
            } else if t < 0.25 {
                DropAt::Before
            } else if t > 0.75 {
                DropAt::After
            } else {
                DropAt::Into
            };
            let stroke = Stroke::new(2.0_f32, theme::ACCENT);
            match at {
                DropAt::Before => {
                    ui.painter().hline(rect.x_range(), rect.top(), stroke);
                }
                DropAt::After => {
                    ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
                }
                DropAt::Into => {
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        3.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
            drop_at = Some(at);
        }
        if response.dnd_hover_payload::<AssetDrag>().is_some()
            || (response.dnd_hover_payload::<ScriptDrag>().is_some() && editor.is_editable(node))
        {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                3.0,
                Stroke::new(2.0_f32, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }

        let indent = 8.0 + depth as f32 * 16.0;
        let painter = ui.painter().clone();
        // Fold arrow.
        let arrow_rect = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(indent + 6.0, 0.0),
            egui::vec2(16.0, 16.0),
        );
        if !children.is_empty() {
            let arrow = if open {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            painter.text(
                arrow_rect.center(),
                egui::Align2::CENTER_CENTER,
                arrow,
                egui::FontId::proportional(12.0),
                theme::TEXT_DIM,
            );
        }
        let arrow_clicked = !children.is_empty()
            && response.clicked()
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| arrow_rect.expand(2.0).contains(p));

        let alpha = if visible { 1.0 } else { 0.45 };
        let class_icon = if instance {
            icon::LINK_SIMPLE
        } else {
            theme::class_icon(class)
        };
        painter.text(
            rect.left_center() + egui::vec2(indent + 24.0, 0.0),
            egui::Align2::CENTER_CENTER,
            class_icon,
            egui::FontId::proportional(13.0),
            theme::class_color(class).gamma_multiply(alpha),
        );

        let text_pos = rect.left_center() + egui::vec2(indent + 36.0, 0.0);
        let renaming_this = self.renaming.as_ref().is_some_and(|(n, _)| *n == node);
        if renaming_this {
            let edit_rect = egui::Rect::from_min_max(
                egui::pos2(text_pos.x - 2.0, rect.top() + 1.0),
                egui::pos2(rect.right() - 50.0, rect.bottom() - 1.0),
            );
            let (_, text) = self.renaming.as_mut().expect("checked");
            let edit = ui.put(
                edit_rect,
                egui::TextEdit::singleline(text).margin(egui::vec2(4.0, 1.0)),
            );
            edit.request_focus();
            if edit.lost_focus() {
                let commit = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                if let Some((node, text)) = self.renaming.take() {
                    if commit {
                        actions.push(Action::Rename(node, text));
                    }
                }
            }
        } else {
            let color = if selected {
                Color32::WHITE
            } else {
                ui.visuals().text_color()
            };
            let galley = painter.layout_no_wrap(
                name.clone(),
                egui::FontId::proportional(13.0),
                color.gamma_multiply(alpha),
            );
            let name_width = galley.size().x;
            painter.galley(
                text_pos - egui::vec2(0.0, galley.size().y / 2.0),
                galley,
                color,
            );
            let mut after = name_width + 8.0;
            if let Some(feature) = feature {
                // Opens in its own editor (the tooltip says how); a UI node says how many
                // controls it holds.
                let hint = match feature {
                    Feature::Ui => format!(
                        "{}  {}",
                        icon::ARROW_SQUARE_OUT,
                        crate::i18n::tf(
                            "features.ui_hint",
                            &[("count", &tree.subtree(node).len().saturating_sub(1))],
                        )
                    ),
                    Feature::Animation
                    | Feature::Cutscene
                    | Feature::AnimationGraph
                    | Feature::Effect => icon::ARROW_SQUARE_OUT.to_owned(),
                };
                let galley =
                    painter.layout_no_wrap(hint, egui::FontId::proportional(11.0), theme::TEXT_DIM);
                let width = galley.size().x;
                painter.galley(
                    text_pos + egui::vec2(after, -galley.size().y / 2.0),
                    galley,
                    theme::TEXT_DIM,
                );
                after += width + 8.0;
            }
            if let Some(script) = &script {
                painter.text(
                    text_pos + egui::vec2(after, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{} {script}", icon::SCROLL),
                    egui::FontId::proportional(11.5),
                    theme::ACCENT.gamma_multiply(0.9),
                );
            }
        }

        // Visibility toggle on the right.
        if node != editor.scene_root && tree.is_node3d(node) {
            let eye_rect = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(16.0, 0.0),
                egui::vec2(18.0, 18.0),
            );
            let eye = if visible { icon::EYE } else { icon::EYE_SLASH };
            let hovered = ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| eye_rect.contains(p));
            painter.text(
                eye_rect.center(),
                egui::Align2::CENTER_CENTER,
                eye,
                egui::FontId::proportional(13.0),
                if hovered {
                    Color32::WHITE
                } else {
                    theme::TEXT_DIM.gamma_multiply(if visible { 0.7 } else { 1.0 })
                },
            );
            if response.clicked() && hovered && !editor.is_playing() {
                actions.push(Action::ToggleVisible(node));
            } else if response.clicked() && !arrow_clicked {
                actions.push(Action::Select(node));
            }
        } else if response.clicked() && !arrow_clicked {
            actions.push(Action::Select(node));
        }

        if arrow_clicked {
            match (prefab, open) {
                (true, true) => _ = self.expanded.remove(&path),
                (true, false) => _ = self.expanded.insert(path.clone()),
                (false, true) => _ = self.collapsed.insert(path.clone()),
                (false, false) => _ = self.collapsed.remove(&path),
            }
        }
        let response = match feature {
            Some(Feature::Ui) => response.on_hover_text(crate::i18n::t("features.ui_open")),
            Some(Feature::Animation) => {
                response.on_hover_text(crate::i18n::t("features.animation_hint"))
            }
            Some(Feature::Cutscene) => {
                response.on_hover_text(crate::i18n::t("features.cutscene_hint"))
            }
            Some(Feature::AnimationGraph) => {
                response.on_hover_text(crate::i18n::t("features.graph_hint"))
            }
            Some(Feature::Effect) => response.on_hover_text(crate::i18n::t("features.effect_hint")),
            None => response,
        };
        if response.double_clicked() {
            if feature.is_some() {
                actions.push(Action::Open(node));
            } else {
                actions.push(Action::Select(node));
                self.frame_request = true;
            }
        }
        if response.secondary_clicked() {
            actions.push(Action::SelectForMenu(node));
        }
        if node != editor.scene_root && !editor.is_playing() && editor.is_editable(node) {
            response.dnd_set_drag_payload(NodeDrag(node));
        }
        if let Some(drag) = response.dnd_release_payload::<NodeDrag>() {
            if let Some(at) = drop_at {
                actions.push(Action::Drop(drag.0, node, at));
            }
        }
        if let Some(asset) = response.dnd_release_payload::<AssetDrag>() {
            actions.push(Action::Instance(asset.0.clone(), node));
        }
        if let Some(script) = response.dnd_release_payload::<ScriptDrag>() {
            actions.push(Action::AttachScript(node, script.0.clone()));
        }

        response.context_menu(|ui| {
            if feature.is_some()
                && ui
                    .button(format!(
                        "{}  {}",
                        icon::ARROW_SQUARE_OUT,
                        crate::i18n::t("features.open")
                    ))
                    .clicked()
            {
                actions.push(Action::Open(node));
                ui.close();
            }
            let editable = !editor.is_playing();
            ui.add_enabled_ui(editable, |ui| {
                if ui
                    .button(format!(
                        "{}  {}",
                        icon::PENCIL_SIMPLE,
                        Command::Rename.label()
                    ))
                    .clicked()
                {
                    self.renaming = Some((node, name.clone()));
                    ui.close();
                }
                let not_root = node != editor.scene_root;
                if not_root
                    && editor.is_editable(node)
                    && !editor.is_instance(node)
                    && ui
                        .button(format!(
                            "{}  {}",
                            icon::LINK_SIMPLE,
                            crate::i18n::t("menu.create_prefab")
                        ))
                        .on_hover_text(crate::i18n::t("scene_tree.prefab_hint"))
                        .clicked()
                {
                    actions.push(Action::MakePrefab(node));
                    ui.close();
                }
                if editor.is_editable(node)
                    && editor
                        .tree()
                        .component::<ab_engine::scene_file::generator::GeneratorNode>(node)
                        .is_some()
                    && ui
                        .button(format!(
                            "{}  {}",
                            icon::CUBE,
                            crate::i18n::t("scene_tree.bake")
                        ))
                        .on_hover_text(crate::i18n::t("scene_tree.bake_hint"))
                        .clicked()
                {
                    actions.push(Action::BakeGenerator(node));
                    ui.close();
                }
                ui.separator();
                commands::menu_item(ui, Command::Cut, not_root);
                commands::menu_item(ui, Command::Copy, not_root);
                commands::menu_item(ui, Command::Paste, not_root && editor.can_paste(None));
                if ui
                    .add_enabled(
                        editor.can_paste(None) && editor.can_add_children(node),
                        egui::Button::new(format!(
                            "{}  {}",
                            icon::CLIPBOARD_TEXT,
                            crate::i18n::t("menu.paste_as_child")
                        )),
                    )
                    .clicked()
                {
                    actions.push(Action::PasteInto(node));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        not_root,
                        egui::Button::new(format!(
                            "{}  {}",
                            icon::COPY,
                            Command::Duplicate.label()
                        ))
                        .shortcut_text("Ctrl+D"),
                    )
                    .clicked()
                {
                    actions.push(Action::Duplicate(node));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        not_root,
                        egui::Button::new(format!(
                            "{}  {}",
                            icon::ARROW_UP,
                            crate::i18n::t("menu.move_up")
                        )),
                    )
                    .clicked()
                {
                    actions.push(Action::Move(node, -1));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        not_root,
                        egui::Button::new(format!(
                            "{}  {}",
                            icon::ARROW_DOWN,
                            crate::i18n::t("menu.move_down")
                        )),
                    )
                    .clicked()
                {
                    actions.push(Action::Move(node, 1));
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        not_root,
                        egui::Button::new(
                            RichText::new(format!("{}  {}", icon::TRASH, Command::Delete.label()))
                                .color(theme::ERROR),
                        )
                        .shortcut_text("Del"),
                    )
                    .clicked()
                {
                    actions.push(Action::Delete(node));
                    ui.close();
                }
            });
            ui.separator();
            if ui
                .button(format!(
                    "{}  {}",
                    icon::CLIPBOARD_TEXT,
                    crate::i18n::t("menu.copy_path")
                ))
                .clicked()
            {
                ui.ctx().copy_text(path.clone());
                ui.close();
            }
        });

        if open {
            for child in children {
                self.row(ui, editor, child, depth + 1, actions);
            }
        }
    }
}

fn paint_row_background(ui: &egui::Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    if selected {
        ui.painter().rect_filled(rect, 3.0, ROW_SELECTED);
    } else if hovered {
        ui.painter().rect_filled(rect, 3.0, theme::BG_3);
    }
}

fn drop_node(editor: &mut Editor, node: NodeHandle, target: NodeHandle, at: DropAt) {
    if node == target {
        return;
    }
    match at {
        DropAt::Into => editor.reparent(node, target, None),
        DropAt::Before | DropAt::After => {
            let Some(parent) = editor.tree().parent(target) else {
                return;
            };
            let siblings = editor.tree().children(parent);
            let Some(mut index) = siblings.iter().position(|&c| c == target) else {
                return;
            };
            // Removing the node from the same parent shifts later siblings.
            if let Some(from) = siblings.iter().position(|&c| c == node) {
                if from < index {
                    index -= 1;
                }
            }
            if at == DropAt::After {
                index += 1;
            }
            editor.reparent(node, parent, Some(index));
        }
    }
}

/// Drops `nodes` on `target` as one undo step, keeping their order, and selects them.
fn drop_nodes(editor: &mut Editor, nodes: &[NodeHandle], target: NodeHandle, at: DropAt) {
    // A node cannot go into itself or into one of its own children.
    let nodes: Vec<NodeHandle> = nodes
        .iter()
        .copied()
        .filter(|&n| n != target && !editor.tree().is_ancestor_of(n, target))
        .collect();
    let active = editor.selection;
    editor.as_one_step(|editor| {
        let (mut target, mut at) = (target, at);
        for &node in &nodes {
            drop_node(editor, node, target, at);
            // The next ones follow it.
            if at != DropAt::Into {
                target = node;
                at = DropAt::After;
            }
        }
    });
    editor.set_selection(nodes, active);
}

/// A click on a row: selects it; Ctrl adds or removes it, Shift selects every row from the
/// active one to it.
fn select_row(
    editor: &mut Editor,
    node: NodeHandle,
    modifiers: egui::Modifiers,
    rows: &[NodeHandle],
) {
    if modifiers.command {
        editor.toggle_selected(node);
    } else if modifiers.shift {
        let anchor = editor
            .selection
            .and_then(|a| rows.iter().position(|&r| r == a));
        match (anchor, rows.iter().position(|&r| r == node)) {
            (Some(a), Some(b)) => {
                let range = rows[a.min(b)..=a.max(b)].to_vec();
                editor.add_to_selection(&range);
                editor.set_selection(editor.selected.clone(), Some(node));
            }
            _ => editor.add_to_selection(&[node]),
        }
    } else {
        editor.select(Some(node));
    }
}

/// Where a new prefab of `node` goes: `prefabs/<name>.abscene`, numbered if taken.
pub fn prefab_path(editor: &Editor, node: NodeHandle) -> String {
    let name: String = editor
        .tree()
        .name(node)
        .map(|n| n.to_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if super::is_name_char(c) || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let ext = ab_engine::scene_file::SCENE_EXTENSION;
    let mut path = format!("prefabs/{name}.{ext}");
    let mut n = 2;
    while editor.project().join(&path).exists() {
        path = format!("prefabs/{name}_{n}.{ext}");
        n += 1;
    }
    path
}
