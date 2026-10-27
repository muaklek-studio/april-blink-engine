//! The command palette (Ctrl+P): one search box for everything the editor can do or open,
//! namely commands, tool windows, nodes to create, the project's files and the scene's
//! nodes. Type a few letters of any word (English or the editor's language), pick with
//! the arrows and Enter.
//!
//! A leading `>` searches only commands and windows, `+` only nodes to create, `@` only
//! the scene's nodes and `/` only files.

use crate::i18n::t;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use egui_phosphor::regular as icon;

/// What choosing an entry does (the app carries it out).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Command(crate::commands::Command),
    /// A tool window, by its key.
    Window(String),
    /// A node to create: an index in [`crate::create::catalog`].
    Create(usize),
    /// A project file.
    Asset(String, crate::project::AssetKind),
    /// A node of the open scene.
    Node(ab_engine::scene::NodeHandle),
}

/// What an entry is, for the filter prefixes and the tag on its right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Command,
    Create,
    Asset,
    Node,
}

impl Group {
    fn tag(self) -> String {
        t(match self {
            Group::Command => "palette.group_command",
            Group::Create => "palette.group_create",
            Group::Asset => "palette.group_asset",
            Group::Node => "palette.group_node",
        })
    }

    /// The group a query's first character limits the search to.
    fn of_prefix(c: char) -> Option<Group> {
        match c {
            '>' => Some(Group::Command),
            '+' => Some(Group::Create),
            '/' => Some(Group::Asset),
            '@' => Some(Group::Node),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub group: Group,
    pub icon: &'static str,
    /// What it is called in the editor's language.
    pub label: String,
    /// Where it is or how to reach it (a path, a menu, a shortcut).
    pub detail: String,
    /// More words it is found by (its English name).
    pub keywords: String,
    pub action: Action,
}

/// How well `query` matches `text`: `None` when its letters do not all appear in order.
/// Matches at word starts and in a row count more; earlier and shorter is better.
pub fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut total = 0;
    let mut at = 0;
    let mut previous: Option<usize> = None;
    for q in query.chars().flat_map(char::to_lowercase) {
        if q.is_whitespace() {
            continue;
        }
        let found = (at..text.len()).find(|&i| text[i] == q)?;
        let word_start = found == 0 || !text[found - 1].is_alphanumeric();
        total += 1;
        if word_start {
            total += 8;
        }
        match previous {
            Some(p) if p + 1 == found => total += 6,
            Some(p) => total -= ((found - p - 1) as i32).min(6),
            None => total -= (found as i32).min(10),
        }
        previous = Some(found);
        at = found + 1;
    }
    Some(total * 4 - text.len() as i32 / 8)
}

/// The palette's state: closed, or open with what was typed.
#[derive(Debug, Default)]
pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    focus: bool,
    /// What was chosen lately, newest first: listed first while nothing is typed.
    recent: Vec<Action>,
}

/// How many recent choices are remembered.
const RECENT: usize = 8;

/// Rows shown at once.
const ROWS: usize = 14;

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens it (empty), or closes it when open.
    pub fn toggle(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.open_with("");
        }
    }

    /// Opens it with `query` typed (`"+"`: the nodes to create).
    pub fn open_with(&mut self, query: &str) {
        self.open = true;
        self.focus = true;
        self.query = query.to_owned();
        self.selected = 0;
    }

    /// The entries matching what is typed, best first.
    pub fn matches<'e>(&self, entries: &'e [Entry]) -> Vec<&'e Entry> {
        let query = self.query.trim_start();
        let (group, query) = match query.chars().next().and_then(Group::of_prefix) {
            Some(group) => (Some(group), query[1..].trim()),
            None => (None, query.trim()),
        };
        let mut found: Vec<(i32, usize, &Entry)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| group.is_none_or(|g| e.group == g))
            .filter_map(|(i, e)| {
                let best = [&e.label, &e.keywords, &e.detail]
                    .iter()
                    .enumerate()
                    // The name matters more than where it is.
                    .filter_map(|(k, text)| score(query, text).map(|s| s - k as i32 * 6))
                    .max()?;
                Some((best, i, e))
            })
            .collect();
        if query.is_empty() {
            // Nothing typed: what was used lately comes first, newest first.
            let rank = |e: &Entry| {
                self.recent
                    .iter()
                    .position(|a| *a == e.action)
                    .unwrap_or(RECENT)
            };
            found.sort_by_key(|(_, i, e)| (rank(e), *i));
        } else {
            found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        found.into_iter().map(|(_, _, e)| e).collect()
    }

    /// Draws it over the window; returns what was chosen (and closes).
    pub fn show(&mut self, ctx: &egui::Context, entries: &[Entry]) -> Option<Action> {
        if !self.open {
            return None;
        }
        let found = self.matches(entries);
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.open = false;
            return None;
        }
        if down && !found.is_empty() {
            self.selected = (self.selected + 1) % found.len();
        }
        if up && !found.is_empty() {
            self.selected = (self.selected + found.len() - 1) % found.len();
        }
        self.selected = self.selected.min(found.len().saturating_sub(1));
        let mut chosen = enter
            .then(|| found.get(self.selected).map(|e| e.action.clone()))
            .flatten();

        let screen = ctx.content_rect();
        let width = (screen.width() - 40.0).clamp(320.0, 640.0);
        let area = egui::Area::new(egui::Id::new("command_palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                screen.center().x - width / 2.0,
                screen.top() + 60.0,
            ));
        let response = area.show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(icon::MAGNIFYING_GLASS)
                                .color(theme::ACCENT)
                                .size(16.0),
                        );
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text(t("palette.hint"))
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Heading),
                        );
                        if std::mem::take(&mut self.focus) {
                            edit.request_focus();
                        }
                        if edit.changed() {
                            self.selected = 0;
                        }
                    });
                    ui.add_space(4.0);
                    if found.is_empty() {
                        ui.label(RichText::new(t("palette.nothing")).color(theme::TEXT_DIM));
                    }
                    let first = self.selected.saturating_sub(ROWS - 1);
                    for (i, entry) in found.iter().enumerate().skip(first).take(ROWS) {
                        let selected = i == self.selected;
                        let row = ui.add(
                            egui::Button::selectable(selected, "")
                                .min_size(egui::vec2(width, 26.0)),
                        );
                        let rect = row.rect;
                        let painter = ui.painter();
                        painter.text(
                            rect.left_center() + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            entry.icon,
                            egui::FontId::proportional(15.0),
                            theme::ACCENT,
                        );
                        painter.text(
                            rect.left_center() + egui::vec2(32.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &entry.label,
                            egui::FontId::proportional(13.5),
                            theme::TEXT,
                        );
                        let tag = format!("{}  ·  {}", entry.detail, entry.group.tag());
                        painter.text(
                            rect.right_center() - egui::vec2(8.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            tag.trim_start_matches("  ·  "),
                            egui::FontId::proportional(11.0),
                            theme::TEXT_DIM,
                        );
                        if row.clicked() {
                            chosen = Some(entry.action.clone());
                        }
                        if row.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            self.selected = i;
                        }
                    }
                    if found.len() > ROWS {
                        ui.label(
                            RichText::new(crate::i18n::tf(
                                "palette.more",
                                &[("count", &(found.len() - ROWS))],
                            ))
                            .small()
                            .color(theme::TEXT_DIM),
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(t("palette.prefixes"))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
        });
        // A click elsewhere closes it.
        if ctx.input(|i| i.pointer.any_pressed())
            && !response
                .response
                .rect
                .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
        {
            self.open = false;
        }
        if let Some(action) = &chosen {
            self.open = false;
            self.recent.retain(|a| a != action);
            self.recent.insert(0, action.clone());
            self.recent.truncate(RECENT);
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(group: Group, label: &str, keywords: &str) -> Entry {
        Entry {
            group,
            icon: "",
            label: label.into(),
            detail: String::new(),
            keywords: keywords.into(),
            action: Action::Window(label.into()),
        }
    }

    #[test]
    fn letters_in_order_match_and_word_starts_win() {
        assert!(score("amx", "Audio Mixer").is_some());
        assert!(score("xma", "Audio Mixer").is_none());
        assert!(score("mix", "Audio Mixer") > score("mix", "Some important xylophone"));
        assert!(score("cube", "Cube") > score("cube", "Cubemap baker"));
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn thai_and_english_names_both_find_an_entry() {
        let entries = [
            entry(Group::Command, "มิกเซอร์เสียง", "Audio Mixer"),
            entry(Group::Create, "Cube", "MeshInstance3D"),
        ];
        let palette = Palette {
            query: "mixer".into(),
            ..Default::default()
        };
        assert_eq!(palette.matches(&entries)[0].label, "มิกเซอร์เสียง");
        let palette = Palette {
            query: "มิกเซอร์".into(),
            ..Default::default()
        };
        assert_eq!(palette.matches(&entries).len(), 1);
    }

    #[test]
    fn with_nothing_typed_recent_choices_come_first() {
        let entries = [
            entry(Group::Command, "Save", ""),
            entry(Group::Command, "Open", ""),
            entry(Group::Command, "Play", ""),
        ];
        let mut palette = Palette {
            recent: vec![Action::Window("Play".into()), Action::Window("Open".into())],
            ..Default::default()
        };
        palette.open_with("");
        let order: Vec<_> = palette
            .matches(&entries)
            .iter()
            .map(|e| e.label.clone())
            .collect();
        assert_eq!(order, ["Play", "Open", "Save"]);
        palette.toggle();
        palette.toggle();
        assert_eq!(palette.recent.len(), 2, "kept when it opens again");
    }

    #[test]
    fn a_prefix_limits_the_search_to_its_group() {
        let entries = [
            entry(Group::Command, "Save", ""),
            entry(Group::Create, "Sphere", ""),
            entry(Group::Node, "Spawner", ""),
        ];
        let search = |q: &str| {
            Palette {
                query: q.into(),
                ..Default::default()
            }
            .matches(&entries)
            .iter()
            .map(|e| e.label.clone())
            .collect::<Vec<_>>()
        };
        assert_eq!(search("+s"), ["Sphere"]);
        assert_eq!(search("@ s"), ["Spawner"]);
        assert_eq!(search(">"), ["Save"]);
        assert_eq!(search("s").len(), 3);
    }
}
