//! Everything around the views: menu bar, start screen, status bar, the
//! hint while files are dragged over the window and the diagnostics window.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use rv_ir::{Diagnostic, Project, Severity};

use super::App;
use super::commands::{Command, OPEN_SHORTCUT, RELOAD_SHORTCUT};

const LIMITS_NOTE: &str = "Syntactic analysis only: macros are not expanded, names and types are not resolved, \
     cfg attributes are not evaluated (cfg(test) code is counted, not shown).";
/// Size of the buttons on the start screen.
const WELCOME_BUTTON: egui::Vec2 = egui::vec2(190.0, 32.0);
/// How long a message stays in the status bar.
const MESSAGE_TIME: Duration = Duration::from_secs(6);

impl App {
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            crate::window_frame::leading_space(ui);
            ui.menu_button("File", |ui| self.file_menu(ui));
            ui.separator();
            match self.loader.path() {
                Some(path) => ui.label(path.display().to_string()),
                None => ui.weak("no project open"),
            };
            if self.loader.is_busy() {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::window_frame::buttons(ui);
                egui::widgets::global_theme_preference_buttons(ui);
                ui.weak("syntactic").on_hover_text(LIMITS_NOTE);
                crate::window_frame::drag_area(ui);
            });
        });
    }

    fn file_menu(&mut self, ui: &mut Ui) {
        let open_keys = ui.ctx().format_shortcut(&OPEN_SHORTCUT);
        let reload_keys = ui.ctx().format_shortcut(&RELOAD_SHORTCUT);
        if ui
            .add(egui::Button::new("Open Project Folder…").shortcut_text(open_keys))
            .clicked()
        {
            self.commands.push(Command::PickFolder);
        }
        if ui.button("Open Rust File…").clicked() {
            self.commands.push(Command::PickFile);
        }
        ui.menu_button("Recent", |ui| self.recent_menu(ui));
        ui.separator();
        let can_reload = self.loader.path().is_some() && !self.loader.is_busy();
        if ui
            .add_enabled(
                can_reload,
                egui::Button::new("Reload").shortcut_text(reload_keys),
            )
            .on_hover_text("Files are reloaded automatically on change.")
            .clicked()
        {
            self.commands.push(Command::Reload);
        }
        ui.separator();
        if ui.button("Quit").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The recently opened projects as a submenu.
    fn recent_menu(&mut self, ui: &mut Ui) {
        if self.recent.is_empty() {
            ui.weak("no recent projects");
        }
        for path in &self.recent {
            let button = egui::Button::new(path.display().to_string());
            if ui.add_enabled(path.exists(), button).clicked() {
                self.commands.push(Command::Open(path.clone()));
            }
        }
        if !self.recent.is_empty() {
            ui.separator();
            if ui.button("Clear List").clicked() {
                self.commands.push(Command::ClearRecent);
            }
        }
    }

    /// Shown when no project is open (or the last one failed to load).
    pub(super) fn welcome(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.2);
            ui.heading(RichText::new("RustView").size(28.0));
            ui.weak("Structure, data flow and parallelism of Rust code at a glance");
            ui.add_space(20.0);
            if let Some(err) = &self.load_error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Could not load: {err}"),
                );
                ui.add_space(12.0);
            }
            if ui
                .add_sized(WELCOME_BUTTON, egui::Button::new("Open Project Folder…"))
                .clicked()
            {
                self.commands.push(Command::PickFolder);
            }
            if ui
                .add_sized(WELCOME_BUTTON, egui::Button::new("Open Rust File…"))
                .clicked()
            {
                self.commands.push(Command::PickFile);
            }
            ui.add_space(8.0);
            ui.weak("…or drop a folder or .rs file onto this window");
            if !self.recent.is_empty() {
                ui.add_space(24.0);
                ui.strong("Recent projects");
                ui.add_space(4.0);
                if let Some(path) = recent_links(ui, &self.recent) {
                    self.commands.push(Command::Open(path));
                }
            }
        });
    }

    /// Hint while files are dragged over the window.
    pub(super) fn drop_overlay(&self, ui: &Ui) {
        if ui.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let rect = ui.ctx().content_rect();
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("drop_overlay"));
        let painter = ui.ctx().layer_painter(layer);
        painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to open",
            egui::FontId::proportional(28.0),
            egui::Color32::WHITE,
        );
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(err) = &self.load_error {
                ui.colored_label(ui.visuals().error_fg_color, format!("load failed: {err}"));
                return;
            }
            let Some(p) = &self.project else {
                if self.loader.path().is_some() {
                    ui.weak("loading…");
                }
                return;
            };
            ui.weak(format!(
                "{} · {} · {} · loaded in {:.2} s",
                plural(p.functions.len(), "function"),
                plural(p.files.len(), "file"),
                plural(p.packages.len(), "package"),
                self.load_time.as_secs_f32()
            ));
            if diagnostics_link(ui, p) {
                self.show_diagnostics = !self.show_diagnostics;
            }
            if let Some(e) = &self.loader.watch_error {
                ui.colored_label(ui.visuals().warn_fg_color, "file watching failed")
                    .on_hover_text(e);
            }
            message_ui(ui, &mut self.message);
        });
    }

    pub(super) fn diagnostics_window(&mut self, ctx: &egui::Context) {
        let Some(project) = &self.project else { return };
        let mut open = self.show_diagnostics;
        let mut goto = None;
        egui::Window::new("Diagnostics")
            .open(&mut open)
            .default_width(600.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for d in &project.diagnostics {
                        if diagnostic_row(ui, project, d) {
                            goto = d.file.clone().map(|file| (file, d.span.clone()));
                        }
                    }
                });
            });
        self.show_diagnostics = open;
        if let Some((path, span)) = goto
            && let Some(id) = project.file_by_path(&path)
        {
            match span {
                Some(s) => self.code.show_span(project, s.clone(), s),
                None => self.code.show_file(project, id),
            }
        }
    }
}

/// The recent projects as links; returns the one that was clicked.
fn recent_links(ui: &mut Ui, recent: &[PathBuf]) -> Option<PathBuf> {
    let mut clicked = None;
    for path in recent {
        let text = path.display().to_string();
        if !path.exists() {
            ui.weak(RichText::new(text).strikethrough())
                .on_hover_text("no longer exists");
        } else if ui.link(text).clicked() {
            clicked = Some(path.clone());
        }
    }
    clicked
}

/// Number of errors and warnings of the project as a link; true when it was clicked.
fn diagnostics_link(ui: &mut Ui, p: &Project) -> bool {
    if p.diagnostics.is_empty() {
        return false;
    }
    let errors = p
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = p.diagnostics.len() - errors;
    let text = RichText::new(format!("⚠ {errors} errors, {warnings} warnings"));
    let text = if errors > 0 {
        text.color(ui.visuals().error_fg_color)
    } else {
        text.color(ui.visuals().warn_fg_color)
    };
    ui.link(text).clicked()
}

/// The latest message, until it is [`MESSAGE_TIME`] old.
fn message_ui(ui: &mut Ui, message: &mut Option<(String, Instant)>) {
    let Some((msg, at)) = message else { return };
    if at.elapsed() < MESSAGE_TIME {
        ui.separator();
        ui.label(&*msg);
    } else {
        *message = None;
    }
}

/// One diagnostic with its file as a link; true when the link was clicked.
fn diagnostic_row(ui: &mut Ui, project: &Project, d: &Diagnostic) -> bool {
    let color = match d.severity {
        Severity::Error => ui.visuals().error_fg_color,
        Severity::Warning => ui.visuals().warn_fg_color,
        Severity::Info => ui.visuals().text_color(),
    };
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color, format!("{:?}", d.severity));
        if let Some(file) = &d.file {
            let shown = file.strip_prefix(&project.root).unwrap_or(file);
            clicked = ui.link(shown.display().to_string()).clicked();
        }
        ui.label(&d.message);
    });
    clicked
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// eframe stores a 0x0 window size when the app is closed while minimized and
/// restores it on the next start. Enlarge such a window once at startup.
pub(super) fn fix_tiny_window(ctx: &egui::Context) {
    const MIN: egui::Vec2 = egui::vec2(640.0, 400.0);
    if ctx.cumulative_frame_nr() > 2 {
        return;
    }
    let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
    if size.is_some_and(|s| s.x < MIN.x || s.y < MIN.y) {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1600.0, 1000.0)));
        // Panel widths etc. were saved for the tiny window as well.
        ctx.data_mut(|d| d.clear());
    }
}
