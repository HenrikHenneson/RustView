//! The application window: its state, and per frame the polling of the
//! loader followed by one call per panel.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::Ui;
use rv_ir::{FnId, Project};

use crate::callgraph_view::CallGraphView;
use crate::code_view::CodeView;
use crate::diagram::DiagramView;
use crate::editor::EditorCommand;
use crate::highlight::Highlighter;
use crate::loader::Loader;
use crate::report_view::ReportView;
use crate::threads::ThreadsView;
use crate::tree::TreeView;

mod chrome;
mod commands;
mod panels;
mod project;

use commands::Command;
use panels::CenterTab;

/// Key of the recent projects in the storage of eframe.
const RECENT_KEY: &str = "recent_projects";

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
    callgraph: CallGraphView,
    report: ReportView,
    center: CenterTab,
    selected: Option<FnId>,
    /// Last selection the tree was expanded to.
    revealed: Option<FnId>,
    tree: TreeView,
    code: CodeView,
    highlighter: Highlighter,
    show_diagnostics: bool,
    /// Shown in the status bar, with the time it was set.
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
            diagram: DiagramView::with_expressions(options.expressions),
            threads: ThreadsView::starting_in(options.threads.as_deref()),
            center: if options.callgraph.is_some() {
                CenterTab::CallGraph
            } else if options.threads.is_some() {
                CenterTab::Threads
            } else {
                CenterTab::Diagram
            },
            callgraph: CallGraphView::opening(options.callgraph.as_deref()),
            report: ReportView::default(),
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

    /// The start screen is shown until a project is loaded or loading.
    fn shows_welcome(&self) -> bool {
        let loading = self.loader.path().is_some() && self.load_error.is_none();
        self.project.is_none() && !loading
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.poll_loader();
        self.handle_input(ui);
        chrome::fix_tiny_window(ui.ctx());
        crate::window_frame::resize_border(ui.ctx());

        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));

        if self.shows_welcome() {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
        } else {
            let tree_action = self.tree_panel(ui);
            let code_action = self.code_panel(ui);
            egui::CentralPanel::default().show(ui, |ui| self.center_panel(ui));
            self.diagnostics_window(ui.ctx());

            self.apply_tree_action(&tree_action);
            self.apply_code_action(code_action);
            self.reveal_selection(ui.ctx(), &tree_action);
        }
        self.drop_overlay(ui);
        self.run_commands();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, RECENT_KEY, &self.recent);
    }
}
