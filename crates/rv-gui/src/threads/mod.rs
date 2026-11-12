//! Project-wide overview of threads and tasks: the program graph and the
//! communication matrix of a program, or one row per spawning function with
//! one card per lane.

use std::collections::HashMap;

use egui::{RichText, Ui};
use rv_analysis::{CallGraph, Facts, LinkKind, Program};
use rv_ir::{FnId, NodeId, Project};

use crate::comm_view::{CommView, Matrix};
use crate::program_view::{GraphLayout, ProgramView};
use crate::style::{CHANNEL, SHARED, SPAWN};
use crate::widgets::{export_button, mono_link};

mod cards;
mod overview;

use overview::Overview;

pub enum ThreadsAction {
    None,
    /// Show this node in the diagram of this function.
    ShowNode(FnId, NodeId),
    ShowFn(FnId),
    /// Status message (e.g. after an export).
    Message(String),
}

impl ThreadsAction {
    /// This action, unless there is a `later` one.
    fn or_later(self, later: ThreadsAction) -> ThreadsAction {
        match later {
            ThreadsAction::None => self,
            later => later,
        }
    }
}

#[derive(Default)]
pub struct ThreadsView {
    cache: Option<(u64, Overview)>,
    mode: Mode,
    program: Option<(u64, ProgramData)>,
    /// Index into the entry points.
    entry: usize,
    view: ProgramView,
    comm: CommView,
    /// Fit the graph on the next frame.
    fit: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Program,
    Communication,
    ByFunction,
}

/// Whole-program analysis, computed once per project version.
struct ProgramData {
    facts: Facts,
    graph: CallGraph,
    entries: Vec<FnId>,
    /// Per entry point, computed on first use.
    programs: HashMap<usize, EntryProgram>,
}

/// The threads of the program starting at one entry point, with what the
/// views show of them.
struct EntryProgram {
    program: Program,
    layout: GraphLayout,
    matrix: Matrix,
}

impl ProgramData {
    fn new(project: &Project) -> Self {
        ProgramData {
            facts: Facts::new(project),
            graph: CallGraph::new(project),
            entries: rv_analysis::entry_points(project),
            programs: HashMap::new(),
        }
    }

    /// The program of the entry point with this index.
    fn entry_program(&mut self, project: &Project, entry: usize) -> &EntryProgram {
        self.programs.entry(entry).or_insert_with(|| {
            let program =
                rv_analysis::program(project, &self.facts, &self.graph, self.entries[entry]);
            EntryProgram {
                layout: crate::program_view::layout(project, &program),
                matrix: Matrix::new(&program),
                program,
            }
        })
    }
}

impl ThreadsView {
    /// `graph`, `communication` or `functions`.
    pub fn starting_in(mode: Option<&str>) -> Self {
        let mode = match mode {
            Some("communication") => Mode::Communication,
            Some("functions") => Mode::ByFunction,
            _ => Mode::Program,
        };
        ThreadsView {
            mode,
            ..ThreadsView::default()
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ThreadsAction {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Program, "program graph")
                .on_hover_text("Threads from main on, with starts, channels and shared state");
            ui.selectable_value(&mut self.mode, Mode::Communication, "communication")
                .on_hover_text("Which thread talks to which: channels, shared state, joins");
            ui.selectable_value(&mut self.mode, Mode::ByFunction, "by function")
                .on_hover_text("Every function that starts threads, with its lanes");
        });
        match self.mode {
            Mode::Program | Mode::Communication => self.program_ui(ui, project, generation),
            Mode::ByFunction => self.by_function_ui(ui, project, generation),
        }
    }

    /// The program graph or the communication matrix of one program.
    fn program_ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ThreadsAction {
        if self.program.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.program = Some((generation, ProgramData::new(project)));
            self.fit = true;
            self.comm.selected = None;
        }
        let entries = &self.program.as_ref().expect("computed above").1.entries;
        if entries.is_empty() {
            ui.weak("No `main` function found (library crate?). Use \"by function\".");
            return ThreadsAction::None;
        }
        self.entry = self.entry.min(entries.len() - 1);
        let export = self.program_toolbar(ui, project);

        let data = &mut self.program.as_mut().expect("computed above").1;
        let entry_fn = data.entries[self.entry];
        let shown = data.entry_program(project, self.entry);
        let mut action = summary_ui(ui, project, entry_fn, &shown.program);
        ui.separator();
        if export && let Some(msg) = export_view(self.mode, project, entry_fn, shown) {
            action = ThreadsAction::Message(msg);
        }
        if self.mode == Mode::Communication {
            let comm = self.comm.ui(ui, project, &shown.program, &shown.matrix);
            return action.or_later(comm);
        }
        if std::mem::take(&mut self.fit) {
            let size = ui.available_size();
            self.view.fit(&shown.layout, size);
            self.view.selected = None;
        }
        let graph = self.view.ui(ui, project, &shown.program, &shown.layout);
        action.or_later(graph)
    }

    /// Choice of the program, export and (for the graph) fit and legend.
    /// True when the export was asked for.
    fn program_toolbar(&mut self, ui: &mut Ui, project: &Project) -> bool {
        let entries = &self.program.as_ref().expect("computed by caller").1.entries;
        let mut export = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("program:");
            if entry_combo(ui, project, entries, &mut self.entry) {
                self.fit = true;
                self.comm.selected = None;
            }
            export = export_button(ui, "Save this view as SVG or PNG");
            if self.mode == Mode::Program {
                if ui.button("Fit").clicked() {
                    self.fit = true;
                }
                ui.separator();
                ui.label(RichText::new("━ starts").color(SPAWN));
                ui.label(RichText::new("━ channel").color(CHANNEL));
                ui.label(RichText::new("┅ shared state").color(SHARED));
                ui.weak("· drag: move · Ctrl+wheel: zoom · double click: show code");
            }
        });
        export
    }

    /// Every function that starts threads, with its lanes.
    fn by_function_ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ThreadsAction {
        if self.cache.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.cache = Some((generation, overview::overview(project)));
        }
        let o = &self.cache.as_ref().expect("computed above").1;
        cards::overview_ui(ui, project, o)
    }
}

/// Choice of the entry point; true when an entry of the list was clicked.
fn entry_combo(ui: &mut Ui, project: &Project, entries: &[FnId], entry: &mut usize) -> bool {
    let mut clicked = false;
    let current = &project.function(entries[*entry]).qual_path;
    egui::ComboBox::from_id_salt("program_entry")
        .selected_text(RichText::new(current).monospace())
        .show_ui(ui, |ui| {
            for (i, &e) in entries.iter().enumerate() {
                let name = RichText::new(&project.function(e).qual_path).monospace();
                clicked |= ui.selectable_value(entry, i, name).clicked();
            }
        });
    clicked
}

/// Numbers of the program and the thread starts that were not reached from
/// its entry point `entry_fn`, as links.
fn summary_ui(ui: &mut Ui, project: &Project, entry_fn: FnId, program: &Program) -> ThreadsAction {
    let mut action = ThreadsAction::None;
    ui.weak(format!(
        "{} threads/tasks · {} channel connections · {} shared-state objects · syntactic call graph from {}",
        program.threads.len(),
        program.links.iter().filter(|l| matches!(l.kind, LinkKind::Channel { .. })).count(),
        program.resources.len(),
        project.function(entry_fn).name
    ));
    if program.unreached.is_empty() {
        return action;
    }
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("{} thread starts not reached from main:", program.unreached.len()),
        )
        .on_hover_text("Called via trait objects, function pointers, macros or calls that could not be resolved unambiguously.");
        for &(f, node) in &program.unreached {
            if mono_link(ui, &project.function(f).qual_path).clicked() {
                action = ThreadsAction::ShowNode(f, node);
            }
        }
    });
    action
}

/// Asks for a file and saves the view of the current mode; returns a status message.
fn export_view(
    mode: Mode,
    project: &Project,
    entry_fn: FnId,
    shown: &EntryProgram,
) -> Option<String> {
    let base = crate::export::file_name(&project.function(entry_fn).qual_path);
    if mode == Mode::Communication {
        crate::export::export(&format!("{base}_communication"), || {
            crate::comm_view::matrix_svg(&shown.program, &shown.matrix)
        })
    } else {
        crate::export::export(&format!("{base}_threads"), || {
            crate::program_view::graph_svg(&shown.layout)
        })
    }
}
