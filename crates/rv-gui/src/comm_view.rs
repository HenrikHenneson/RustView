//! How threads communicate: a thread × thread matrix (row → column: what
//! flows from the row thread to the column thread) and the details of the
//! selected channel, shared object or join.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Color32, RichText, Ui};
use rv_analysis::{Program, Site};
use rv_ir::Project;

use crate::threads::ThreadsAction;

const CHANNEL: Color32 = Color32::from_rgb(0, 150, 165);
const SHARED: Color32 = Color32::from_rgb(150, 90, 210);
const JOIN: Color32 = Color32::from_rgb(210, 110, 0);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum CommRef {
    Channel(usize),
    Shared(usize),
    Join(usize),
}

#[derive(Default)]
pub struct CommView {
    pub selected: Option<CommRef>,
}

/// Operations that change shared state (or may: unknown methods).
fn writes(op: &str) -> bool {
    let method = op.rsplit('.').next().unwrap_or(op);
    !matches!(method, "load" | "read" | "try_read" | "is_poisoned") && !op.starts_with("stored in")
}

fn thread_label(program: &Program, t: usize) -> String {
    format!("#{t} {}", program.threads[t].title())
}

/// What happens at an operation, in words.
fn semantics(op: Option<&str>, bounded: bool) -> String {
    let Some(op) = op else {
        return "handed on or captured (not followed further)".into();
    };
    if let Some(field) = op.strip_prefix("stored in ") {
        return format!("kept in the field {field}; its methods use it");
    }
    // `stand.lock`: the operation is on a field of the shared object.
    let op = op.rsplit('.').next().unwrap_or(op);
    let s = match op {
        "send" | "blocking_send" | "send_blocking" if bounded => {
            "sends; blocks while the channel is full"
        }
        "send" => "sends; does not block (unbounded)",
        "try_send" => "sends if there is room; fails when full (message lost unless handled)",
        "send_timeout" => "sends; waits at most a timeout when full",
        "recv" | "blocking_recv" | "recv_async" => "waits until a message arrives",
        "try_recv" => "polls; returns at once if nothing is there",
        "recv_timeout" | "recv_deadline" => "waits for a message, at most a timeout",
        "iter" => "loops over messages, waiting for each",
        "try_iter" => "takes what is there without waiting",
        "lock" | "write" => "exclusive lock; waits while another thread holds it",
        "try_lock" | "try_write" => "tries to lock; does not wait",
        "read" => "shared read lock; waits while a writer holds it",
        "load" => "atomic read",
        "store" => "atomic write",
        "swap" | "compare_exchange" | "compare_exchange_weak" | "fetch_update" => {
            "atomic read-modify-write"
        }
        "join" => "waits until the thread has finished",
        "notify_one" | "notify_all" => "wakes waiting threads",
        "wait" | "wait_timeout" | "wait_while" => "sleeps until notified",
        op if op.starts_with("fetch_") => "atomic read-modify-write",
        _ => "method on the object (what it does inside is not analyzed)",
    };
    s.to_string()
}

/// Cells of the matrix: (from, to) -> communication objects.
fn cells(program: &Program) -> BTreeMap<(usize, usize), BTreeSet<CommRef>> {
    let mut cells: BTreeMap<(usize, usize), BTreeSet<CommRef>> = BTreeMap::new();
    for (i, c) in program.channels.iter().enumerate() {
        for s in c.senders.iter().map(|s| s.thread) {
            for r in c.receivers.iter().map(|s| s.thread) {
                if s != r {
                    cells.entry((s, r)).or_default().insert(CommRef::Channel(i));
                }
            }
        }
    }
    for (i, sh) in program.shared.iter().enumerate() {
        let writers: BTreeSet<usize> = sh
            .users
            .iter()
            .filter(|s| s.op.as_deref().is_some_and(writes))
            .map(|s| s.thread)
            .collect();
        for &w in &writers {
            for u in sh.threads() {
                if w != u {
                    cells.entry((w, u)).or_default().insert(CommRef::Shared(i));
                }
            }
        }
    }
    for (i, j) in program.joins.iter().enumerate() {
        for w in j.waiters.iter().map(|s| s.thread) {
            if w != j.thread {
                cells
                    .entry((j.thread, w))
                    .or_default()
                    .insert(CommRef::Join(i));
            }
        }
    }
    cells
}

/// `#0, #2`
fn thread_set(set: impl IntoIterator<Item = usize>) -> String {
    let v: BTreeSet<usize> = set.into_iter().collect();
    v.iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Hover text of a chip: what it is and where it comes from.
fn chip_tooltip(project: &Project, program: &Program, c: CommRef) -> String {
    let origin = |(f, _): (rv_ir::FnId, rv_ir::NodeId)| project.function(f).qual_path.clone();
    match c {
        CommRef::Channel(i) => {
            let ch = &program.channels[i];
            format!(
                "channel {}
{}
message: {}
created in {}",
                ch.name,
                ch.api,
                ch.message.as_deref().unwrap_or("?"),
                origin(ch.site)
            )
        }
        CommRef::Shared(i) => {
            let sh = &program.shared[i];
            format!(
                "{} ({})
created in {}
used by {}",
                sh.name,
                sh.kind,
                origin(sh.site),
                thread_set(sh.threads())
            )
        }
        CommRef::Join(i) => format!(
            "waits for {} to finish",
            program.threads[program.joins[i].thread].title()
        ),
    }
}

fn chip(program: &Program, c: CommRef) -> (String, Color32) {
    match c {
        CommRef::Channel(i) => (program.channels[i].name.clone(), CHANNEL),
        CommRef::Shared(i) => (program.shared[i].name.clone(), SHARED),
        CommRef::Join(_) => ("join".into(), JOIN),
    }
}

impl CommView {
    pub fn ui(&mut self, ui: &mut Ui, project: &Project, program: &Program) -> ThreadsAction {
        let mut action = ThreadsAction::None;
        let cells = cells(program);
        let threads: BTreeSet<usize> = cells.keys().flat_map(|&(a, b)| [a, b]).collect();
        if threads.is_empty() {
            ui.weak("No communication between threads found (no channels, shared objects or joins across threads).");
            return action;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("row → column:");
            ui.label(RichText::new("■ channel (sends to)").color(CHANNEL));
            ui.label(RichText::new("■ shared state (changes, the other uses)").color(SHARED));
            ui.label(RichText::new("■ join (column waits for row to end)").color(JOIN));
        });
        ui.separator();
        // Details in a panel on the right whose width the user can drag;
        // the matrix takes the rest.
        let total = ui.available_width();
        egui::Panel::right("comm_details_panel")
            .resizable(true)
            .default_size(total * 0.4)
            .size_range(220.0..=(total - 150.0).max(240.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("comm_details")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(a) = self.details(ui, project, program) {
                            action = a;
                        }
                    });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            // Chips must not wrap into narrow columns.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            egui::ScrollArea::both()
                .id_salt("comm_matrix")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("comm_grid")
                        .striped(true)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.weak("from \\ to");
                            for &c in &threads {
                                ui.strong(format!("#{c}"))
                                    .on_hover_text(program.threads[c].title());
                            }
                            ui.end_row();
                            for &r in &threads {
                                ui.label(RichText::new(thread_label(program, r)).strong());
                                for &c in &threads {
                                    if r == c {
                                        ui.weak("·");
                                        continue;
                                    }
                                    ui.vertical(|ui| {
                                        for &comm in cells.get(&(r, c)).into_iter().flatten() {
                                            let (text, color) = chip(program, comm);
                                            let selected = self.selected == Some(comm);
                                            let label =
                                                RichText::new(text).color(color).monospace();
                                            if ui
                                                .selectable_label(selected, label)
                                                .on_hover_text(chip_tooltip(project, program, comm))
                                                .clicked()
                                            {
                                                self.selected = Some(comm);
                                            }
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                });
        });
        action
    }

    fn details(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        program: &Program,
    ) -> Option<ThreadsAction> {
        let mut action = None;
        match self.selected {
            None => {
                ui.heading("Communication objects");
                ui.weak("Click an entry here or in the matrix for details.");
                if !program.channels.is_empty() {
                    ui.add_space(6.0);
                    ui.strong(format!("Channels ({})", program.channels.len()));
                    for (i, c) in program.channels.iter().enumerate() {
                        let from: BTreeSet<usize> = c.senders.iter().map(|s| s.thread).collect();
                        let to: BTreeSet<usize> = c.receivers.iter().map(|s| s.thread).collect();
                        let text = format!("{}  {} → {}", c.name, thread_set(from), thread_set(to));
                        if ui
                            .link(RichText::new(text).color(CHANNEL).monospace())
                            .clicked()
                        {
                            self.selected = Some(CommRef::Channel(i));
                        }
                    }
                }
                if !program.shared.is_empty() {
                    ui.add_space(6.0);
                    ui.strong(format!("Shared state ({})", program.shared.len()));
                    for (i, s) in program.shared.iter().enumerate() {
                        let text = format!("{} ({})  {}", s.name, s.kind, thread_set(s.threads()));
                        if ui
                            .link(RichText::new(text).color(SHARED).monospace())
                            .clicked()
                        {
                            self.selected = Some(CommRef::Shared(i));
                        }
                    }
                }
                if !program.joins.is_empty() {
                    ui.add_space(6.0);
                    ui.strong(format!("Joins ({})", program.joins.len()));
                    for (i, j) in program.joins.iter().enumerate() {
                        let text = format!("waits for {}", thread_label(program, j.thread));
                        if ui.link(RichText::new(text).color(JOIN)).clicked() {
                            self.selected = Some(CommRef::Join(i));
                        }
                    }
                }
            }
            Some(CommRef::Channel(i)) => {
                let c = &program.channels[i];
                ui.horizontal(|ui| {
                    if ui.small_button("← all").clicked() {
                        self.selected = None;
                    }
                    ui.heading(RichText::new(&c.name).color(CHANNEL));
                });
                ui.monospace(&c.api);
                action = action.or(site_link(ui, project, c.site));
                ui.label(format!(
                    "message type: {}",
                    c.message.as_deref().unwrap_or("(not written in the code)")
                ));
                let bounded = c.capacity.is_some();
                ui.label(match &c.capacity {
                    Some(n) => format!("bounded: {n} messages"),
                    None => "unbounded (or capacity not visible)".into(),
                });
                ui.add_space(6.0);
                ui.strong("sends");
                action = action.or(site_list(ui, project, program, &c.senders, bounded));
                ui.add_space(6.0);
                ui.strong("receives");
                action = action.or(site_list(ui, project, program, &c.receivers, bounded));
            }
            Some(CommRef::Shared(i)) => {
                let s = &program.shared[i];
                ui.horizontal(|ui| {
                    if ui.small_button("← all").clicked() {
                        self.selected = None;
                    }
                    ui.heading(RichText::new(format!("{} ({})", s.name, s.kind)).color(SHARED));
                });
                action = action.or(site_link(ui, project, s.site));
                ui.add_space(6.0);
                ui.strong("used by");
                action = action.or(site_list(ui, project, program, &s.users, false));
            }
            Some(CommRef::Join(i)) => {
                let j = &program.joins[i];
                ui.horizontal(|ui| {
                    if ui.small_button("← all").clicked() {
                        self.selected = None;
                    }
                    ui.heading(
                        RichText::new(format!("join {}", program.threads[j.thread].title()))
                            .color(JOIN),
                    );
                });
                ui.strong("waiting");
                action = action.or(site_list(ui, project, program, &j.waiters, false));
            }
        }
        action
    }
}

/// Places where an object is used, grouped by thread; clicking one shows it.
fn site_list(
    ui: &mut Ui,
    project: &Project,
    program: &Program,
    sites: &[Site],
    bounded: bool,
) -> Option<ThreadsAction> {
    let mut action = None;
    let mut by_thread: BTreeMap<usize, Vec<&Site>> = BTreeMap::new();
    for s in sites {
        by_thread.entry(s.thread).or_default().push(s);
    }
    for (t, list) in by_thread {
        ui.label(RichText::new(program.threads[t].title()).strong());
        ui.indent(("sites", t), |ui| {
            for s in list {
                let func = project.function(s.func);
                let line = func
                    .body
                    .as_ref()
                    .map_or(0, |b| b.node(s.node).span.start.line);
                ui.horizontal_wrapped(|ui| {
                    let link = ui.link(RichText::new(format!("{}:{line}", func.name)).monospace());
                    if link.on_hover_text(&func.qual_path).clicked() {
                        action = Some(ThreadsAction::ShowNode(s.func, s.node));
                    }
                    if let Some(op) = &s.op {
                        ui.monospace(op);
                    }
                    ui.weak(semantics(s.op.as_deref(), bounded));
                });
            }
        });
    }
    action
}

fn site_link(
    ui: &mut Ui,
    project: &Project,
    (f, node): (rv_ir::FnId, rv_ir::NodeId),
) -> Option<ThreadsAction> {
    let func = project.function(f);
    let line = func
        .body
        .as_ref()
        .map_or(0, |b| b.node(node).span.start.line);
    ui.link(format!("created in {}:{line}", func.qual_path))
        .clicked()
        .then_some(ThreadsAction::ShowNode(f, node))
}
