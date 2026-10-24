//! Editor commands and their keyboard shortcuts, in one table.
//!
//! Menus, context menus, the toolbar and the keyboard all name a [`Command`]; the app runs
//! it ([`crate::app::EditorApp`]). Each binding has a [`Scope`] that says when its keys
//! count, so that:
//!
//! * play controls (F5, F6, F7) work even while the game has the keyboard,
//! * editing keys (Ctrl+Z, Ctrl+C, Delete, Ctrl+A, …) are left to a text field that has
//!   focus, which then undoes, copies or selects its own text,
//! * a key runs one command once per press: the press is consumed by the first match.
//!
//! Copy, cut and paste arrive from the platform as clipboard events rather than key presses
//! (see [`pressed`]).

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use egui_phosphor::regular as icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    NewScene,
    OpenScene,
    Save,
    SaveAs,
    ExportGame,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Delete,
    Rename,
    SelectAll,
    SelectNone,
    FrameSelection,
    Play,
    Stop,
    Pause,
    StepFrame,
    ToolSelect,
    ToolMove,
    ToolRotate,
    ToolScale,
    ToggleSnap,
    ToggleHighlight,
    Shortcuts,
    Preferences,
    OpenUiDesigner,
    OpenAnimationEditor,
    OpenAssetBrowser,
    OpenGit,
}

/// When a binding's keys count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Always, even while the game has the keyboard.
    Global,
    /// While editing (not while the game has the keyboard), even in a text field.
    Editor,
    /// Like [`Scope::Editor`], except while a text field has the keyboard: the field
    /// handles these keys itself.
    Scene,
}

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub command: Command,
    pub shortcut: KeyboardShortcut,
    pub scope: Scope,
}

const fn bind(command: Command, modifiers: Modifiers, key: Key, scope: Scope) -> Binding {
    Binding {
        command,
        shortcut: KeyboardShortcut::new(modifiers, key),
        scope,
    }
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const NONE: Modifiers = Modifiers::NONE;

/// Every shortcut, most specific first (Ctrl+Shift+S before Ctrl+S). A command may have
/// several; the first is the one menus show.
pub const BINDINGS: &[Binding] = &[
    bind(Command::Stop, Modifiers::SHIFT, Key::F5, Scope::Global),
    bind(Command::Play, NONE, Key::F5, Scope::Global),
    bind(Command::Pause, NONE, Key::F6, Scope::Global),
    bind(Command::StepFrame, NONE, Key::F7, Scope::Global),
    bind(Command::SaveAs, CTRL_SHIFT, Key::S, Scope::Editor),
    bind(Command::ExportGame, CTRL_SHIFT, Key::E, Scope::Editor),
    bind(Command::Redo, CTRL_SHIFT, Key::Z, Scope::Scene),
    bind(
        Command::OpenUiDesigner,
        CTRL_SHIFT,
        Key::Num1,
        Scope::Editor,
    ),
    bind(
        Command::OpenAnimationEditor,
        CTRL_SHIFT,
        Key::Num2,
        Scope::Editor,
    ),
    bind(
        Command::OpenAssetBrowser,
        CTRL_SHIFT,
        Key::Num3,
        Scope::Editor,
    ),
    bind(Command::OpenGit, CTRL_SHIFT, Key::Num4, Scope::Editor),
    bind(Command::Save, CTRL, Key::S, Scope::Editor),
    bind(Command::NewScene, CTRL, Key::N, Scope::Editor),
    bind(Command::OpenScene, CTRL, Key::O, Scope::Editor),
    bind(Command::Undo, CTRL, Key::Z, Scope::Scene),
    bind(Command::Redo, CTRL, Key::Y, Scope::Scene),
    bind(Command::Cut, CTRL, Key::X, Scope::Scene),
    bind(Command::Copy, CTRL, Key::C, Scope::Scene),
    bind(Command::Paste, CTRL, Key::V, Scope::Scene),
    bind(Command::Duplicate, CTRL, Key::D, Scope::Scene),
    bind(Command::SelectAll, CTRL, Key::A, Scope::Scene),
    bind(Command::Delete, NONE, Key::Delete, Scope::Scene),
    bind(Command::SelectNone, NONE, Key::Escape, Scope::Scene),
    bind(Command::Rename, NONE, Key::F2, Scope::Scene),
    bind(Command::Shortcuts, NONE, Key::F1, Scope::Editor),
    bind(Command::Preferences, CTRL, Key::Comma, Scope::Editor),
    bind(Command::ToolSelect, NONE, Key::Q, Scope::Scene),
    bind(Command::ToolMove, NONE, Key::W, Scope::Scene),
    bind(Command::ToolRotate, NONE, Key::E, Scope::Scene),
    bind(Command::ToolScale, NONE, Key::R, Scope::Scene),
    bind(Command::ToggleSnap, NONE, Key::G, Scope::Scene),
    bind(Command::ToggleHighlight, NONE, Key::H, Scope::Scene),
];

impl Command {
    /// The command's name in the language files (`command.<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Command::NewScene => "new_scene",
            Command::OpenScene => "open_scene",
            Command::Save => "save",
            Command::SaveAs => "save_as",
            Command::ExportGame => "export_game",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Cut => "cut",
            Command::Copy => "copy",
            Command::Paste => "paste",
            Command::Duplicate => "duplicate",
            Command::Delete => "delete",
            Command::Rename => "rename",
            Command::SelectAll => "select_all",
            Command::SelectNone => "select_none",
            Command::FrameSelection => "frame_selection",
            Command::Play => "play",
            Command::Stop => "stop",
            Command::Pause => "pause",
            Command::StepFrame => "step_frame",
            Command::ToolSelect => "tool_select",
            Command::ToolMove => "tool_move",
            Command::ToolRotate => "tool_rotate",
            Command::ToolScale => "tool_scale",
            Command::ToggleSnap => "toggle_snap",
            Command::ToggleHighlight => "toggle_highlight",
            Command::Shortcuts => "shortcuts",
            Command::Preferences => "preferences",
            Command::OpenUiDesigner => "open_ui_designer",
            Command::OpenAnimationEditor => "open_animation_editor",
            Command::OpenAssetBrowser => "open_asset_browser",
            Command::OpenGit => "open_git",
        }
    }

    pub fn text_key(self) -> String {
        format!("command.{}", self.name())
    }

    /// The command's name in the editor's language.
    pub fn label(self) -> String {
        crate::i18n::t(&self.text_key())
    }

    pub fn icon(self) -> &'static str {
        match self {
            Command::NewScene => icon::FILE_PLUS,
            Command::OpenScene => icon::FOLDER_OPEN,
            Command::Save => icon::FLOPPY_DISK,
            Command::SaveAs => icon::FLOPPY_DISK_BACK,
            Command::ExportGame => icon::PACKAGE,
            Command::Undo => icon::ARROW_U_UP_LEFT,
            Command::Redo => icon::ARROW_U_UP_RIGHT,
            Command::Cut => icon::SCISSORS,
            Command::Copy => icon::COPY_SIMPLE,
            Command::Paste => icon::CLIPBOARD,
            Command::Duplicate => icon::COPY,
            Command::Delete => icon::TRASH,
            Command::Rename => icon::PENCIL_SIMPLE,
            Command::SelectAll => icon::SELECTION_ALL,
            Command::SelectNone => icon::SELECTION_SLASH,
            Command::FrameSelection => icon::CROSSHAIR,
            Command::Play => icon::PLAY,
            Command::Stop => icon::STOP,
            Command::Pause => icon::PAUSE,
            Command::StepFrame => icon::SKIP_FORWARD,
            Command::ToolSelect => icon::CURSOR,
            Command::ToolMove => icon::ARROWS_OUT_CARDINAL,
            Command::ToolRotate => icon::ARROWS_CLOCKWISE,
            Command::ToolScale => icon::ARROWS_OUT,
            Command::ToggleSnap => icon::MAGNET,
            Command::ToggleHighlight => icon::SELECTION,
            Command::Shortcuts => icon::KEYBOARD,
            Command::Preferences => icon::GEAR_SIX,
            Command::OpenUiDesigner => icon::BROWSERS,
            Command::OpenAnimationEditor => icon::FILM_STRIP,
            Command::OpenAssetBrowser => icon::FOLDERS,
            Command::OpenGit => icon::GIT_BRANCH,
        }
    }

    /// The first shortcut of the command.
    pub fn shortcut(self) -> Option<KeyboardShortcut> {
        BINDINGS
            .iter()
            .find(|b| b.command == self)
            .map(|b| b.shortcut)
    }

    /// A tooltip: the label and its shortcut (`Save (Ctrl+S)`).
    pub fn tooltip(self, ctx: &egui::Context) -> String {
        match self.shortcut_text(ctx) {
            keys if keys.is_empty() => self.label(),
            keys => format!("{} ({keys})", self.label()),
        }
    }

    /// The shortcut as menus show it (`Ctrl+S`, `⌘S` on macOS), or nothing.
    pub fn shortcut_text(self, ctx: &egui::Context) -> String {
        match self {
            Command::FrameSelection => "F".to_owned(),
            _ => self
                .shortcut()
                .map(|s| ctx.format_shortcut(&s))
                .unwrap_or_default(),
        }
    }
}

/// Where the keyboard is: whether the game has it, and whether a text field does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub game: bool,
    pub text: bool,
}

impl Focus {
    fn allows(self, scope: Scope) -> bool {
        match scope {
            Scope::Global => true,
            Scope::Editor => !self.game,
            Scope::Scene => !self.game && !self.text,
        }
    }
}

/// Commands whose keys were pressed this frame, each press consumed so nothing else acts
/// on it. Copy, cut and paste come as clipboard events (the platform turns Ctrl+C and the
/// Copy key into them); a paste carries the clipboard's text.
pub fn pressed(ctx: &egui::Context, focus: Focus) -> Vec<(Command, Option<String>)> {
    let mut commands = Vec::new();
    ctx.input_mut(|input| {
        for binding in BINDINGS {
            if focus.allows(binding.scope) && input.consume_shortcut(&binding.shortcut) {
                commands.push((binding.command, None));
            }
        }
        if focus.allows(Scope::Scene) {
            input.events.retain(|event| {
                let command = match event {
                    egui::Event::Copy => (Command::Copy, None),
                    egui::Event::Cut => (Command::Cut, None),
                    egui::Event::Paste(text) => (Command::Paste, Some(text.clone())),
                    _ => return true,
                };
                commands.push(command);
                false
            });
        }
    });
    commands
}

const REQUESTS: &str = "ab-editor.command-requests";

/// Asks the app to run `command` at the end of the frame (from a panel or context menu).
pub fn request(ctx: &egui::Context, command: Command) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<Command>>(egui::Id::new(REQUESTS))
            .push(command);
    });
}

/// The commands requested this frame.
pub fn take_requests(ctx: &egui::Context) -> Vec<Command> {
    ctx.data_mut(|d| d.remove_temp::<Vec<Command>>(egui::Id::new(REQUESTS)))
        .unwrap_or_default()
}

/// A menu entry for `command`: icon, label, shortcut. Requests the command when clicked.
pub fn menu_item(ui: &mut egui::Ui, command: Command, enabled: bool) -> bool {
    let text = command.shortcut_text(ui.ctx());
    let button =
        egui::Button::new(format!("{}  {}", command.icon(), command.label())).shortcut_text(text);
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        request(ui.ctx(), command);
        ui.close();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(ctx: &egui::Context, modifiers: Modifiers, key: Key, focus: Focus) -> Vec<Command> {
        let mut found = Vec::new();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            found = pressed(ui.ctx(), focus)
                .into_iter()
                .map(|(c, _)| c)
                .collect();
        });
        output.textures_delta.clear();
        found
    }

    const EDITING: Focus = Focus {
        game: false,
        text: false,
    };

    #[test]
    fn one_press_one_command() {
        let ctx = egui::Context::default();
        assert_eq!(press(&ctx, CTRL, Key::S, EDITING), [Command::Save]);
        // Ctrl+Shift+S is Save As only, not Save as well.
        assert_eq!(press(&ctx, CTRL_SHIFT, Key::S, EDITING), [Command::SaveAs]);
        assert_eq!(press(&ctx, CTRL_SHIFT, Key::Z, EDITING), [Command::Redo]);
        assert_eq!(press(&ctx, CTRL, Key::Z, EDITING), [Command::Undo]);
        assert_eq!(press(&ctx, NONE, Key::Delete, EDITING), [Command::Delete]);
    }

    #[test]
    fn text_fields_keep_their_editing_keys() {
        let ctx = egui::Context::default();
        let typing = Focus {
            game: false,
            text: true,
        };
        assert!(press(&ctx, CTRL, Key::Z, typing).is_empty());
        assert!(press(&ctx, CTRL, Key::A, typing).is_empty());
        assert!(press(&ctx, NONE, Key::Delete, typing).is_empty());
        assert!(press(&ctx, NONE, Key::W, typing).is_empty());
        // Saving still works from a text field.
        assert_eq!(press(&ctx, CTRL, Key::S, typing), [Command::Save]);
    }

    #[test]
    fn the_game_keeps_the_keyboard_but_play_controls_work() {
        let ctx = egui::Context::default();
        let playing = Focus {
            game: true,
            text: false,
        };
        assert!(press(&ctx, CTRL, Key::S, playing).is_empty());
        assert!(press(&ctx, NONE, Key::W, playing).is_empty());
        assert_eq!(press(&ctx, NONE, Key::F5, playing), [Command::Play]);
        assert_eq!(
            press(&ctx, Modifiers::SHIFT, Key::F5, playing),
            [Command::Stop]
        );
    }

    #[test]
    fn clipboard_events_become_commands() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Copy, egui::Event::Paste("text".into())],
            ..Default::default()
        };
        let mut found = Vec::new();
        let mut output = ctx.run_ui(input, |ui| found = pressed(ui.ctx(), EDITING));
        output.textures_delta.clear();
        assert_eq!(
            found,
            [(Command::Copy, None), (Command::Paste, Some("text".into()))]
        );
    }

    #[test]
    fn no_shortcut_is_bound_twice() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                assert!(
                    a.shortcut != b.shortcut,
                    "{:?} and {:?} share a shortcut",
                    a.command,
                    b.command
                );
            }
        }
    }
}
