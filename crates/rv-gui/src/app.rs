use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Key, KeyboardShortcut, Modifiers, RichText, Ui};
use rv_ir::{FnId, Project, Severity};

use crate::code_view::{CodeAction, CodeView};
use crate::diagram::{DiagramAction, DiagramView};
use crate::editor::EditorCommand;
use crate::highlight::Highlighter;
use crate::loader::Loader;
use crate::threads::{ThreadsAction, ThreadsView};
use crate::tree::{TreeAction, TreeView};

const RECENT_KEY: &str = "recent_projects";
const MAX_RECENT: usize = 10;
const OPEN_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const RELOAD_SHORTCUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);

const LIMITS_NOTE: &str = "Syntactic analysis only: macros are not expanded, names and types are not resolved, \
     cfg attributes are not evaluated (cfg(test) code is counted, not shown).";

#[derive(Clone, Copy, PartialEq, Eq)]
enum CenterTab {
    Diagram,
    Threads,
}

/// Actions collected while drawing and executed at the end of the frame.
enum Command {
    PickFolder,
    PickFile,
    Open(PathBuf),
    Reload,
    ClearRecent,
}

pub struct App {
    ctx: egui::Context,
    loader: Loader,
    /// Most recently opened first.
    recent: Vec<PathBuf>,
    commands: Vec<Command>,
    editor: EditorCommand,
    project: Option<Project>,
    load_error: Option<String>,
    load_time: Duration,
    /// Incremented for every adopted project version (invalidates layouts).
    generation: u64,
    diagram: DiagramView,
    threads: ThreadsView,
    center: CenterTab,
    selected: Option<FnId>,
    /// Last selection the tree was expanded to.
    revealed: Option<FnId>,
    tree: TreeView,
    code: CodeView,
    highlighter: Highlighter,
    show_diagnostics: bool,
    message: Option<(String, Instant)>,
    /// From `--select`, applied after the first load.
    initial_selection: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, options: crate::Options) -> Self {
        let recent = cc
            .storage
            .and_then(|s| eframe::get_value::<Vec<PathBuf>>(s, RECENT_KEY))
            .unwrap_or_default();
        let mut app = App {
            ctx: cc.egui_ctx.clone(),
            loader: Loader::new(cc.egui_ctx.clone()),
            recent,
            commands: Vec::new(),
            editor: options.editor,
            initial_selection: options.select,
            project: None,
            load_error: None,
            load_time: Duration::ZERO,
            generation: 0,
            diagram: DiagramView::default(),
            threads: ThreadsView::starting_in(options.threads.as_deref()),
            center: if options.threads.is_some() {
                CenterTab::Threads
            } else {
                CenterTab::Diagram
            },
            selected: None,
            revealed: None,
            tree: TreeView::default(),
            code: CodeView::default(),
            highlighter: Highlighter::new(cc.egui_ctx.clone()),
            show_diagnostics: false,
            message: None,
        };
        if let Some(path) = options.path {
            app.open_project(path);
        }
        app
    }

    /// Switch to another project (directory with Cargo.toml or a .rs file).
    fn open_project(&mut self, path: PathBuf) {
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

    fn run_commands(&mut self) {
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

    fn poll_loader(&mut self) {
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
            (Some((path, line)), _) => {
                // Same path can exist twice (e.g. a file in two targets): prefer the closest line.
                project
                    .functions
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.qual_path == path)
                    .min_by_key(|(_, f)| f.span.start.line.abs_diff(line))
                    .map(|(i, _)| FnId(i as u32))
            }
            (None, Some(wanted)) => {
                let found = find_function(&project, &wanted);
                if found.is_none() {
                    self.message =
                        Some((format!("no function matches `{wanted}`"), Instant::now()));
                }
                found
            }
            (None, None) => None,
        };

        let file = open_file.and_then(|p| project.file_by_path(&p));
        match (self.selected, file) {
            // On reload keep the scroll position instead of jumping back.
            (Some(f), Some(file)) if is_reload && project.function(f).span.file == file => {
                let func = project.function(f);
                let (region, mark) = (func.span.clone(), func.name_span.clone());
                self.code
                    .update_in_place(&project, file, Some(region), Some(mark));
            }
            (Some(f), _) => {
                let func = project.function(f);
                self.code
                    .show_span(&project, func.span.clone(), func.name_span.clone());
            }
            (None, Some(file)) if is_reload => {
                self.code.update_in_place(&project, file, None, None)
            }
            (None, Some(file)) => self.code.show_file(&project, file),
            (None, None) => self.code.clear(),
        }
        self.code.cursor_line = cursor;
        self.project = Some(project);
    }

    fn select(&mut self, f: FnId) {
        let Some(project) = &self.project else { return };
        if self.selected != Some(f) {
            self.diagram.function_changed();
        }
        self.selected = Some(f);
        let func = project.function(f);
        self.code
            .show_span(project, func.span.clone(), func.name_span.clone());
    }

    fn open_in_editor(&mut self, file: PathBuf, line: u32, col: u32) {
        let msg = match self.editor.open(&file, line, col) {
            Ok(()) => format!("opened {}:{line}", file.display()),
            Err(e) => format!("could not open editor: {e:#}"),
        };
        self.message = Some((msg, Instant::now()));
    }

    fn open_fn_in_editor(&mut self, f: FnId) {
        let Some(project) = &self.project else { return };
        let func = project.function(f);
        let path = project.file(func.name_span.file).path.clone();
        let (line, col) = (func.name_span.start.line, func.name_span.start.col);
        self.open_in_editor(path, line, col);
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
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
                egui::widgets::global_theme_preference_buttons(ui);
                ui.weak("syntactic").on_hover_text(LIMITS_NOTE);
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
        ui.menu_button("Recent", |ui| {
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
        });
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

    /// Shown when no project is open (or the last one failed to load).
    fn welcome(&mut self, ui: &mut Ui) {
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
            let size = egui::vec2(190.0, 32.0);
            if ui
                .add_sized(size, egui::Button::new("Open Project Folder…"))
                .clicked()
            {
                self.commands.push(Command::PickFolder);
            }
            if ui
                .add_sized(size, egui::Button::new("Open Rust File…"))
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
                for path in &self.recent {
                    let text = path.display().to_string();
                    if path.exists() {
                        if ui.link(text).clicked() {
                            self.commands.push(Command::Open(path.clone()));
                        }
                    } else {
                        ui.weak(RichText::new(text).strikethrough())
                            .on_hover_text("no longer exists");
                    }
                }
            }
        });
    }

    fn handle_input(&mut self, ui: &mut Ui) {
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

    /// Hint while files are dragged over the window.
    fn drop_overlay(&self, ui: &Ui) {
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

    fn status_bar(&mut self, ui: &mut Ui) {
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
            let errors = p
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count();
            let warnings = p.diagnostics.len() - errors;
            if !p.diagnostics.is_empty() {
                let text = RichText::new(format!("⚠ {errors} errors, {warnings} warnings"));
                let text = if errors > 0 {
                    text.color(ui.visuals().error_fg_color)
                } else {
                    text.color(ui.visuals().warn_fg_color)
                };
                if ui.link(text).clicked() {
                    self.show_diagnostics = !self.show_diagnostics;
                }
            }
            if let Some(e) = &self.loader.watch_error {
                ui.colored_label(ui.visuals().warn_fg_color, "file watching failed")
                    .on_hover_text(e);
            }
            if let Some((msg, at)) = &self.message {
                if at.elapsed() < Duration::from_secs(6) {
                    ui.separator();
                    ui.label(msg);
                } else {
                    self.message = None;
                }
            }
        });
    }

    fn center_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.center, CenterTab::Diagram, "Function diagram");
            ui.selectable_value(&mut self.center, CenterTab::Threads, "Threads")
                .on_hover_text("All threads and tasks of the project");
        });
        ui.separator();
        match self.center {
            CenterTab::Diagram => self.diagram_panel(ui),
            CenterTab::Threads => self.threads_panel(ui),
        }
    }

    fn threads_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        match self.threads.ui(ui, project, self.generation) {
            ThreadsAction::None => {}
            ThreadsAction::ShowFn(f) => {
                self.select(f);
                self.center = CenterTab::Diagram;
            }
            ThreadsAction::ShowNode(f, node) => {
                self.select(f);
                if let Some(project) = &self.project {
                    self.diagram.reveal_node(project, f, node);
                    if let Some(body) = &project.function(f).body {
                        let n = body.node(node);
                        self.code.show_span(project, n.span.clone(), n.head.clone());
                    }
                }
                self.center = CenterTab::Diagram;
            }
        }
    }

    fn diagram_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        let Some(f) = self.selected else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Select a function on the left");
                ui.weak("Click: show diagram and source · Double click: open in editor");
            });
            return;
        };
        let action = self.diagram.ui(ui, project, self.generation, f);
        let Some(body) = &project.function(f).body else {
            return;
        };
        match action {
            DiagramAction::None => {}
            DiagramAction::Select(n) => {
                let node = body.node(n);
                self.code
                    .show_span(project, node.span.clone(), node.head.clone());
            }
            DiagramAction::Open(n) => {
                let head = body.node(n).head.clone();
                let path = project.file(head.file).path.clone();
                self.open_in_editor(path, head.start.line, head.start.col);
            }
        }
    }

    fn diagnostics_window(&mut self, ctx: &egui::Context) {
        let Some(project) = &self.project else { return };
        let mut open = self.show_diagnostics;
        let mut goto = None;
        egui::Window::new("Diagnostics")
            .open(&mut open)
            .default_width(600.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for d in &project.diagnostics {
                        let color = match d.severity {
                            Severity::Error => ui.visuals().error_fg_color,
                            Severity::Warning => ui.visuals().warn_fg_color,
                            Severity::Info => ui.visuals().text_color(),
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(color, format!("{:?}", d.severity));
                            if let Some(file) = &d.file {
                                let shown = file.strip_prefix(&project.root).unwrap_or(file);
                                if ui.link(shown.display().to_string()).clicked() {
                                    goto = Some((file.clone(), d.span.clone()));
                                }
                            }
                            ui.label(&d.message);
                        });
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.poll_loader();
        self.handle_input(ui);
        fix_tiny_window(ui.ctx());

        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));

        // Start screen until a project is loaded or loading.
        let loading = self.loader.path().is_some() && self.load_error.is_none();
        if self.project.is_none() && !loading {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
            self.drop_overlay(ui);
            self.run_commands();
            return;
        }

        let mut tree_action = TreeAction::None;
        // Panels remember the width their content used, so the content has
        // to claim the full width or the panel shrinks to fit it.
        egui::Panel::left("tree_panel")
            .resizable(true)
            .default_size(340.0)
            .min_size(180.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                match &self.project {
                    Some(p) => tree_action = self.tree.ui(ui, p, self.selected),
                    None => {
                        ui.spinner();
                    }
                }
            });

        let mut code_action = CodeAction::None;
        egui::Panel::right("code_panel")
            .resizable(true)
            .default_size(760.0)
            .min_size(300.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(p) = &self.project {
                    code_action = self.code.ui(ui, p, &mut self.highlighter);
                }
            });

        egui::CentralPanel::default().show(ui, |ui| self.center_panel(ui));

        let ctx = ui.ctx().clone();
        self.diagnostics_window(&ctx);

        match tree_action {
            TreeAction::None => {}
            TreeAction::Select(f) => self.select(f),
            TreeAction::Open(f) => {
                self.select(f);
                self.open_fn_in_editor(f);
            }
            TreeAction::OpenFile(file) => {
                if let Some(p) = &self.project {
                    self.code.show_file(p, file);
                }
            }
        }
        match code_action {
            CodeAction::None => {}
            CodeAction::Clicked { line } => {
                // Show the innermost statement of the clicked line in the diagram.
                if let (Some(p), Some(f), Some(file)) =
                    (&self.project, self.selected, self.code.file)
                {
                    self.diagram.reveal_line(p, f, file, line);
                }
            }
            CodeAction::OpenInEditor { line, col } => {
                if let Some((p, file)) = self.project.as_ref().zip(self.code.file) {
                    let path = p.file(file).path.clone();
                    self.open_in_editor(path, line, col);
                }
            }
        }
        // Selections made outside the tree (e.g. after a reload) become visible in it.
        if self.selected != self.revealed {
            if let (Some(p), Some(f)) = (&self.project, self.selected)
                && !matches!(tree_action, TreeAction::Select(_) | TreeAction::Open(_))
            {
                self.tree.reveal(ui.ctx(), p, f);
            }
            self.revealed = self.selected;
        }
        self.drop_overlay(ui);
        self.run_commands();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, RECENT_KEY, &self.recent);
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Exact qualified path, or the shortest one ending in `::wanted`.
fn find_function(project: &Project, wanted: &str) -> Option<FnId> {
    let suffix = format!("::{wanted}");
    project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path == wanted || f.qual_path.ends_with(&suffix))
        .min_by_key(|(_, f)| f.qual_path.len())
        .map(|(i, _)| FnId(i as u32))
}

/// eframe stores a 0x0 window size when the app is closed while minimized and
/// restores it on the next start. Enlarge such a window once at startup.
fn fix_tiny_window(ctx: &egui::Context) {
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
