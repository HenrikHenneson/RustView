//! What the user asks the application to do (menu, shortcuts, dropped
//! files): collected while drawing and executed at the end of the frame.

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers, Ui};

use super::App;

pub(super) const OPEN_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
pub(super) const RELOAD_SHORTCUT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::F5);

/// Actions collected while drawing and executed at the end of the frame.
pub(super) enum Command {
    PickFolder,
    PickFile,
    Open(PathBuf),
    Reload,
    ClearRecent,
}

impl App {
    /// Shortcuts and files dropped onto the window.
    pub(super) fn handle_input(&mut self, ui: &mut Ui) {
        if ui.input_mut(|i| i.consume_shortcut(&OPEN_SHORTCUT)) {
            self.commands.push(Command::PickFolder);
        }
        if ui.input_mut(|i| i.consume_shortcut(&RELOAD_SHORTCUT)) && self.loader.path().is_some() {
            self.commands.push(Command::Reload);
        }
        let dropped = ui.input(|i| {
            i.raw.dropped_files.iter().find_map(|f| {
                let path = f.path();
                (!path.as_os_str().is_empty()).then(|| path.to_path_buf())
            })
        });
        if let Some(path) = dropped {
            self.commands.push(Command::Open(path));
        }
    }

    pub(super) fn run_commands(&mut self) {
        for command in std::mem::take(&mut self.commands) {
            match command {
                Command::PickFolder => {
                    let picked = self
                        .dialog()
                        .set_title("Open Rust project folder")
                        .pick_folder();
                    if let Some(dir) = picked {
                        self.open_project(dir);
                    }
                }
                Command::PickFile => {
                    let picked = self
                        .dialog()
                        .set_title("Open Rust file")
                        .add_filter("Rust source or Cargo.toml", &["rs", "toml"])
                        .pick_file();
                    if let Some(file) = picked {
                        self.open_project(file);
                    }
                }
                Command::Open(path) => self.open_project(path),
                Command::Reload => self.loader.reload(),
                Command::ClearRecent => self.recent.clear(),
            }
        }
    }

    /// File dialog starting next to the current or last project.
    fn dialog(&self) -> rfd::FileDialog {
        let start = self
            .loader
            .path()
            .or(self.recent.first().map(PathBuf::as_path))
            .and_then(|p| p.parent());
        match start {
            Some(dir) => rfd::FileDialog::new().set_directory(dir),
            None => rfd::FileDialog::new(),
        }
    }
}
