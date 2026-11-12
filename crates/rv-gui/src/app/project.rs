//! The open project and what is selected in it: opening, adopting a
//! (re)loaded version, selecting functions and jumping to the editor.

use std::path::PathBuf;
use std::time::Instant;

use rv_ir::{FileId, FnId, Project};

use super::App;
use crate::lookup::{find_function, function_near, innermost_function_at};
use crate::tree::TreeView;

/// Length of the list of recently opened projects.
const MAX_RECENT: usize = 10;

impl App {
    /// Switch to another project (directory with Cargo.toml or a .rs file).
    pub(super) fn open_project(&mut self, path: PathBuf) {
        let path = rv_syn::normalize_path(&path);
        let title = format!("RustView – {}", path.display());
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
        self.loader.open(path);
        self.project = None;
        self.load_error = None;
        self.selected = None;
        self.revealed = None;
        self.tree = TreeView::default();
        self.diagram.reset();
        self.code.clear();
        self.highlighter.clear();
        self.show_diagnostics = false;
    }

    fn remember_recent(&mut self, path: PathBuf) {
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    /// Takes over a project the loader has finished since the last frame.
    pub(super) fn poll_loader(&mut self) {
        let Some(loaded) = self.loader.poll() else {
            return;
        };
        match loaded.result {
            Ok(project) => {
                self.remember_recent(loaded.path);
                self.load_time = loaded.elapsed;
                self.load_error = None;
                self.adopt(project);
            }
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    /// Replace the project, keeping the selection (by qualified path) if possible.
    fn adopt(&mut self, project: Project) {
        let old = self.project.take();
        let previous = old.as_ref().zip(self.selected).map(|(p, f)| {
            let func = p.function(f);
            (func.qual_path.clone(), func.span.start.line)
        });
        let open_file = old
            .as_ref()
            .zip(self.code.file)
            .map(|(p, f)| p.file(f).path.clone());
        let cursor = self.code.cursor_line;

        self.highlighter.clear();
        self.generation += 1;
        let is_reload = old.is_some();
        self.selected = match (previous, self.initial_selection.take()) {
            (Some((path, line)), _) => function_near(&project, &path, line),
            (None, Some(wanted)) => {
                let found = find_function(&project, &wanted);
                if found.is_none() {
                    self.show_message(format!("no function matches `{wanted}`"));
                }
                found
            }
            (None, None) => None,
        };

        let file = open_file.and_then(|p| project.file_by_path(&p));
        self.show_code_after_load(&project, file, is_reload);
        self.code.cursor_line = cursor;
        self.project = Some(project);
    }

    /// Fills the code panel for a newly adopted project: the selected
    /// function, or else the `file` that was open before.
    fn show_code_after_load(&mut self, project: &Project, file: Option<FileId>, is_reload: bool) {
        match (self.selected, file) {
            // On reload keep the scroll position instead of jumping back.
            (Some(f), Some(file)) if is_reload && project.function(f).span.file == file => {
                let func = project.function(f);
                let (region, mark) = (func.span.clone(), func.name_span.clone());
                self.code
                    .update_in_place(project, file, Some(region), Some(mark));
            }
            (Some(f), _) => {
                let func = project.function(f);
                self.code
                    .show_span(project, func.span.clone(), func.name_span.clone());
            }
            (None, Some(file)) if is_reload => self.code.update_in_place(project, file, None, None),
            (None, Some(file)) => self.code.show_file(project, file),
            (None, None) => self.code.clear(),
        }
    }

    pub(super) fn select(&mut self, f: FnId) {
        let Some(project) = &self.project else { return };
        if self.selected != Some(f) {
            self.diagram.function_changed();
        }
        self.selected = Some(f);
        let func = project.function(f);
        self.code
            .show_span(project, func.span.clone(), func.name_span.clone());
    }

    /// Shows a line of a file in the code panel and, if it is inside a
    /// function, selects the innermost one and shows the line in its diagram.
    pub(super) fn goto_line(&mut self, file: FileId, line: u32) {
        let Some(project) = &self.project else { return };
        let inner = innermost_function_at(project, file, line);
        if let Some(f) = inner {
            self.select(f);
        }
        let Some(project) = &self.project else { return };
        if let Some(f) = inner {
            self.diagram.reveal_line(project, f, file, line);
        }
        self.code.show_line(project, file, line);
    }

    pub(super) fn open_in_editor(&mut self, file: PathBuf, line: u32, col: u32) {
        let msg = match self.editor.open(&file, line, col) {
            Ok(()) => format!("opened {}:{line}", file.display()),
            Err(e) => format!("could not open editor: {e:#}"),
        };
        self.show_message(msg);
    }

    pub(super) fn open_fn_in_editor(&mut self, f: FnId) {
        let Some(project) = &self.project else { return };
        let func = project.function(f);
        let path = project.file(func.name_span.file).path.clone();
        let (line, col) = (func.name_span.start.line, func.name_span.start.col);
        self.open_in_editor(path, line, col);
    }

    /// Shows a message in the status bar for a few seconds.
    pub(super) fn show_message(&mut self, message: String) {
        self.message = Some((message, Instant::now()));
    }
}
