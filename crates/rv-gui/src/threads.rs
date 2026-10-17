//! Project-wide overview of threads and tasks: one row per spawning function,
//! one card per lane, with the channels and shared state each lane touches.

use std::collections::HashMap;

use egui::{Color32, RichText, Stroke, Ui};
use rv_analysis::{CallGraph, Concurrency, End, Facts, Program};
use rv_ir::{Body, FnId, NodeId, Project};

use crate::comm_view::CommView;
use crate::program_view::{GraphLayout, ProgramView};

pub enum ThreadsAction {
    None,
    /// Show this node in the diagram of this function.
    ShowNode(FnId, NodeId),
    ShowFn(FnId),
    /// Status message (e.g. after an export).
    Message(String),
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
    programs: HashMap<usize, (Program, GraphLayout)>,
}

struct Overview {
    groups: Vec<Group>,
    channels: usize,
    shared: usize,
    lanes: usize,
}

/// A function that starts lanes.
struct Group {
    func: FnId,
    /// The function itself (the spawning thread) and its lanes.
    cards: Vec<Card>,
}

struct Card {
    title: String,
    node: Option<NodeId>,
    line: u32,
    parallel: bool,
    runs: Vec<(String, Option<FnId>)>,
    /// (channel name, color index, end, operation)
    channels: Vec<(String, usize, End, Option<String>)>,
    shared: Vec<String>,
}

const CHANNEL_COLORS: [Color32; 6] = [
    Color32::from_rgb(0, 150, 165),
    Color32::from_rgb(210, 110, 0),
    Color32::from_rgb(60, 110, 220),
    Color32::from_rgb(190, 60, 140),
    Color32::from_rgb(90, 140, 30),
    Color32::from_rgb(120, 90, 200),
];
const CARD_WIDTH: f32 = 280.0;

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

    fn program_ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ThreadsAction {
        let mut action = ThreadsAction::None;
        if self.program.as_ref().is_none_or(|(g, _)| *g != generation) {
            let entries = rv_analysis::entry_points(project);
            self.program = Some((
                generation,
                ProgramData {
                    facts: Facts::new(project),
                    graph: CallGraph::new(project),
                    entries,
                    programs: HashMap::new(),
                },
            ));
            self.fit = true;
            self.comm.selected = None;
        }
        let data = &mut self.program.as_mut().expect("computed above").1;
        if data.entries.is_empty() {
            ui.weak("No `main` function found (library crate?). Use \"by function\".");
            return action;
        }
        self.entry = self.entry.min(data.entries.len() - 1);

        let mut export = false;
        ui.horizontal_wrapped(|ui| {
            let current = &project.function(data.entries[self.entry]).qual_path;
            ui.label("program:");
            egui::ComboBox::from_id_salt("program_entry")
                .selected_text(RichText::new(current).monospace())
                .show_ui(ui, |ui| {
                    for (i, &e) in data.entries.iter().enumerate() {
                        if ui
                            .selectable_value(
                                &mut self.entry,
                                i,
                                RichText::new(&project.function(e).qual_path).monospace(),
                            )
                            .clicked()
                        {
                            self.fit = true;
                            self.comm.selected = None;
                        }
                    }
                });
            if ui
                .button("Export…")
                .on_hover_text("Save this view as SVG or PNG")
                .clicked()
            {
                export = true;
            }
            if self.mode == Mode::Program {
                if ui.button("Fit").clicked() {
                    self.fit = true;
                }
                ui.separator();
                ui.label(RichText::new("━ starts").color(Color32::from_rgb(130, 130, 130)));
                ui.label(RichText::new("━ channel").color(Color32::from_rgb(0, 150, 165)));
                ui.label(RichText::new("┅ shared state").color(Color32::from_rgb(150, 90, 210)));
                ui.weak("· drag: move · Ctrl+wheel: zoom · double click: show code");
            }
        });

        let entry = self.entry;
        let (program, layout) = data.programs.entry(entry).or_insert_with(|| {
            let p = rv_analysis::program(project, &data.facts, &data.graph, data.entries[entry]);
            let l = crate::program_view::layout(project, &p);
            (p, l)
        });
        ui.weak(format!(
            "{} threads/tasks · {} channel connections · {} shared-state objects · syntactic call graph from {}",
            program.threads.len(),
            program.links.iter().filter(|l| matches!(l.kind, rv_analysis::LinkKind::Channel { .. })).count(),
            program.resources.len(),
            project.function(data.entries[entry]).name
        ));
        if !program.unreached.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{} thread starts not reached from main:", program.unreached.len()),
                )
                .on_hover_text("Called via trait objects, function pointers, macros or calls that could not be resolved unambiguously.");
                for &(f, node) in &program.unreached {
                    if ui.link(RichText::new(&project.function(f).qual_path).monospace()).clicked() {
                        action = ThreadsAction::ShowNode(f, node);
                    }
                }
            });
        }
        ui.separator();
        if export {
            let base = crate::export::file_name(&project.function(data.entries[entry]).qual_path);
            let (name, svg) = if self.mode == Mode::Communication {
                (
                    format!("{base}_communication"),
                    crate::comm_view::svg(program),
                )
            } else {
                (
                    format!("{base}_threads"),
                    crate::program_view::svg(project, program, layout),
                )
            };
            if let Some(msg) = crate::export::export(&name, || svg) {
                action = ThreadsAction::Message(msg);
            }
        }
        if self.mode == Mode::Communication {
            return match self.comm.ui(ui, project, program) {
                ThreadsAction::None => action,
                other => other,
            };
        }
        if std::mem::take(&mut self.fit) {
            let size = ui.available_size();
            self.view.fit(layout, size);
            self.view.selected = None;
        }
        match self.view.ui(ui, project, program, layout) {
            ThreadsAction::None => {}
            other => action = other,
        }
        action
    }

    fn by_function_ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ThreadsAction {
        if self.cache.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.cache = Some((generation, overview(project)));
        }
        let o = &self.cache.as_ref().expect("computed above").1;
        let mut action = ThreadsAction::None;

        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "{} threads/tasks started in {} functions · {} channels · {} shared-state groups",
                o.lanes,
                o.groups.len(),
                o.channels,
                o.shared
            ));
            ui.weak("· recognized by API name (thread::spawn/Builder, scope, tokio/smol/async-std, rayon, join!/select!)");
        });
        ui.separator();
        if o.groups.is_empty() {
            ui.weak("No threads or tasks found.");
            return action;
        }

        egui::ScrollArea::vertical()
            .id_salt("threads_scroll")
            .show(ui, |ui| {
                for g in &o.groups {
                    let func = project.function(g.func);
                    ui.horizontal(|ui| {
                        if ui
                            .link(RichText::new(&func.qual_path).monospace().strong())
                            .on_hover_text("Show the diagram")
                            .clicked()
                        {
                            action = ThreadsAction::ShowFn(g.func);
                        }
                        ui.weak(format!("line {}", func.name_span.start.line));
                    });
                    egui::ScrollArea::horizontal()
                        .id_salt(("threads_row", g.func))
                        .show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                for (i, card) in g.cards.iter().enumerate() {
                                    if i == 1 {
                                        ui.label(RichText::new("‖").size(28.0).weak());
                                    }
                                    if let Some(a) = card_ui(ui, g.func, card) {
                                        action = a;
                                    }
                                }
                            });
                        });
                    ui.add_space(12.0);
                }
            });
        action
    }
}

fn card_ui(ui: &mut Ui, func: FnId, card: &Card) -> Option<ThreadsAction> {
    let mut action = None;
    let stroke = if card.parallel {
        Stroke::new(2.0, Color32::from_rgb(0, 160, 120))
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    egui::Frame::group(ui.style())
        .stroke(stroke)
        .inner_margin(8.0)
        .show(ui, |ui| {
            // The row is horizontal; the card's content is a column.
            ui.vertical(|ui| {
                ui.set_width(CARD_WIDTH);
                ui.strong(&card.title);
                if let Some(node) = card.node
                    && ui.link(format!("started at line {}", card.line)).clicked()
                {
                    action = Some(ThreadsAction::ShowNode(func, node));
                }
                if !card.runs.is_empty() {
                    ui.add_space(4.0);
                    ui.weak("runs");
                    for (name, target) in card.runs.iter().take(6) {
                        match target {
                            Some(f) => {
                                if ui.link(RichText::new(name).monospace()).clicked() {
                                    action = Some(ThreadsAction::ShowFn(*f));
                                }
                            }
                            None => {
                                ui.label(RichText::new(name).monospace().weak());
                            }
                        }
                    }
                }
                if !card.channels.is_empty() {
                    ui.add_space(4.0);
                    ui.weak("channels");
                    for (name, color, end, op) in &card.channels {
                        let arrow = match end {
                            End::Send => "→ sends to",
                            End::Recv => "← receives from",
                        };
                        let op = op
                            .as_deref()
                            .map_or_else(|| " (passed on)".to_string(), |o| format!(" ({o})"));
                        let color = CHANNEL_COLORS[color % CHANNEL_COLORS.len()];
                        ui.label(RichText::new(format!("{arrow} {name}{op}")).color(color));
                    }
                }
                if !card.shared.is_empty() {
                    ui.add_space(4.0);
                    ui.weak("shared state");
                    for name in &card.shared {
                        ui.label(
                            RichText::new(name)
                                .monospace()
                                .color(Color32::from_rgb(150, 90, 210)),
                        );
                    }
                }
            });
        });
    action
}

fn overview(project: &Project) -> Overview {
    let mut o = Overview {
        groups: Vec::new(),
        channels: 0,
        shared: 0,
        lanes: 0,
    };
    for (i, func) in project.functions.iter().enumerate() {
        let Some(body) = &func.body else { continue };
        let flow = rv_analysis::data_flow(body);
        let c = rv_analysis::concurrency(body, &flow, &project.type_aliases);
        if !c.spawns.iter().any(|s| s.kind.is_parallel()) {
            continue;
        }
        let f = FnId(i as u32);
        let crate_name = func.qual_path.split("::").next().unwrap_or("");
        let mut cards = vec![Card {
            title: format!("{} (this thread)", func.name),
            node: None,
            line: func.name_span.start.line,
            parallel: false,
            runs: Vec::new(),
            channels: lane_channels(body, &c, None),
            shared: lane_shared(body, &c, None),
        }];
        for (si, s) in c
            .spawns
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind.is_parallel())
        {
            o.lanes += 1;
            let mut title = s.kind.label().to_string();
            if let Some(name) = &s.name {
                title.push_str(&format!(" '{name}'"));
            }
            if s.in_loop {
                title.push_str(" ×n");
            }
            cards.push(Card {
                title,
                node: Some(s.node),
                line: body.node(s.node).span.start.line,
                parallel: true,
                runs: s
                    .runs
                    .iter()
                    .map(|r| (r.clone(), resolve(project, crate_name, r)))
                    .collect(),
                channels: lane_channels(body, &c, Some(si)),
                shared: lane_shared(body, &c, Some(si)),
            });
        }
        o.channels += c.channels.len();
        o.shared += c.shared.len();
        o.groups.push(Group { func: f, cards });
    }
    o
}

/// Lane of a node for the overview: the spawn statement itself belongs to
/// its (inline) lane if it has no structure lanes.
fn lane(body: &Body, c: &Concurrency, node: NodeId) -> Option<usize> {
    c.lane_of(body, node).or_else(|| {
        c.spawns
            .iter()
            .position(|s| s.node == node && s.lanes.is_empty() && s.kind.is_parallel())
    })
}

fn lane_channels(
    body: &Body,
    c: &Concurrency,
    l: Option<usize>,
) -> Vec<(String, usize, End, Option<String>)> {
    let mut out: Vec<(String, usize, End, Option<String>)> = Vec::new();
    for u in &c.channel_uses {
        if lane(body, c, u.node) != l {
            continue;
        }
        let entry = (
            c.channels[u.channel].name.clone(),
            u.channel,
            u.end,
            u.op.clone(),
        );
        if !out.iter().any(|e| e.1 == entry.1 && e.2 == entry.2) {
            out.push(entry);
        }
    }
    out
}

fn lane_shared(body: &Body, c: &Concurrency, l: Option<usize>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in &c.shared_uses {
        if lane(body, c, u.node) != l {
            continue;
        }
        let g = &c.shared[u.group];
        let name = format!("{} ({})", g.name, g.kind);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Project function a called path refers to, preferring the caller's crate.
fn resolve(project: &Project, crate_name: &str, path: &str) -> Option<FnId> {
    let path = path
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .trim_start_matches("super::");
    let suffix = format!("::{path}");
    let candidates: Vec<usize> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path.ends_with(&suffix))
        .map(|(i, _)| i)
        .collect();
    candidates
        .iter()
        .find(|&&i| project.functions[i].qual_path.starts_with(crate_name))
        .or(candidates.first())
        .map(|&i| FnId(i as u32))
}
