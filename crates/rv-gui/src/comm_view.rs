//! How threads communicate: a thread × thread matrix (row → column: what
//! flows from the row thread to the column thread) and the details of the
//! selected channel, shared object or join.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Color32, Rect, RichText, Ui};
use rv_analysis::{Program, Site, op_meaning as semantics, op_writes as writes};
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
fn thread_label(program: &Program, t: usize) -> String {
    format!("#{t} {}", program.threads[t].title())
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

/// Lines between the matrix cells and a gray diagonal (a thread with itself).
fn draw_grid_lines(ui: &Ui, cells: &[(usize, usize, Rect)], n: usize) {
    if cells.is_empty() {
        return;
    }
    let mut col = vec![(f32::INFINITY, f32::NEG_INFINITY); n];
    let mut row = vec![(f32::INFINITY, f32::NEG_INFINITY); n];
    for &(r, c, rect) in cells {
        col[c] = (col[c].0.min(rect.min.x), col[c].1.max(rect.max.x));
        row[r] = (row[r].0.min(rect.min.y), row[r].1.max(rect.max.y));
    }
    // Boundaries halfway between neighboring columns / rows.
    let bounds = |v: &[(f32, f32)]| -> Vec<f32> {
        let mut b = vec![v[0].0 - 6.0];
        for w in v.windows(2) {
            b.push((w[0].1 + w[1].0) / 2.0);
        }
        b.push(v[v.len() - 1].1 + 6.0);
        b
    };
    let (xs, ys) = (bounds(&col), bounds(&row));
    let painter = ui.painter();
    let gray = ui.visuals().weak_text_color().gamma_multiply(0.25);
    for i in 1..n {
        // The thread with itself.
        let r = Rect::from_min_max(egui::pos2(xs[i], ys[i]), egui::pos2(xs[i + 1], ys[i + 1]));
        painter.rect_filled(r, 0.0, gray);
    }
    let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    for &x in &xs {
        painter.line_segment([egui::pos2(x, ys[0]), egui::pos2(x, ys[n])], stroke);
    }
    for &y in &ys {
        painter.line_segment([egui::pos2(xs[0], y), egui::pos2(xs[n], y)], stroke);
    }
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
                    // Cell rects by (row, column), 0 = header; for the lines drawn afterwards.
                    let mut cell_rects: Vec<(usize, usize, Rect)> = Vec::new();
                    egui::Grid::new("comm_grid")
                        .spacing([14.0, 8.0])
                        .show(ui, |ui| {
                            let r = ui.weak("from → to").rect;
                            cell_rects.push((0, 0, r));
                            for (ci, &c) in threads.iter().enumerate() {
                                let r = ui
                                    .strong(format!("#{c}"))
                                    .on_hover_text(program.threads[c].title())
                                    .rect;
                                cell_rects.push((0, ci + 1, r));
                            }
                            ui.end_row();
                            for (ri, &r) in threads.iter().enumerate() {
                                let rect = ui
                                    .label(RichText::new(thread_label(program, r)).strong())
                                    .rect;
                                cell_rects.push((ri + 1, 0, rect));
                                for (ci, &c) in threads.iter().enumerate() {
                                    let rect = ui
                                        .vertical(|ui| {
                                            for &comm in cells.get(&(r, c)).into_iter().flatten() {
                                                let (text, color) = chip(program, comm);
                                                let selected = self.selected == Some(comm);
                                                let label =
                                                    RichText::new(text).color(color).monospace();
                                                if ui
                                                    .selectable_label(selected, label)
                                                    .on_hover_text(chip_tooltip(
                                                        project, program, comm,
                                                    ))
                                                    .clicked()
                                                {
                                                    self.selected = Some(comm);
                                                }
                                            }
                                        })
                                        .response
                                        .rect;
                                    cell_rects.push((ri + 1, ci + 1, rect));
                                }
                                ui.end_row();
                            }
                        });
                    draw_grid_lines(ui, &cell_rects, threads.len() + 1);
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

/// The communication matrix as SVG table.
pub fn svg(program: &Program) -> String {
    use crate::export::Svg;
    use egui::{pos2, vec2};
    const CH: f32 = 7.0;
    const LINE: f32 = 16.0;
    const PAD: f32 = 8.0;
    let cells = cells(program);
    let threads: Vec<usize> = cells
        .keys()
        .flat_map(|&(a, b)| [a, b])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let head_w = threads
        .iter()
        .map(|&t| thread_label(program, t).chars().count())
        .max()
        .unwrap_or(10) as f32
        * CH
        + 2.0 * PAD;
    let col_w: Vec<f32> = threads
        .iter()
        .map(|&c| {
            let widest = threads
                .iter()
                .flat_map(|&r| cells.get(&(r, c)).into_iter().flatten())
                .map(|&x| chip(program, x).0.chars().count())
                .max()
                .unwrap_or(3);
            widest.max(3) as f32 * CH + 2.0 * PAD
        })
        .collect();
    let row_h: Vec<f32> = threads
        .iter()
        .map(|&r| {
            let most = threads
                .iter()
                .map(|&c| cells.get(&(r, c)).map_or(0, |s| s.len()))
                .max()
                .unwrap_or(0);
            most.max(1) as f32 * LINE + PAD
        })
        .collect();
    let width = head_w + col_w.iter().sum::<f32>();
    let height = LINE + PAD + row_h.iter().sum::<f32>();
    let mut svg = Svg::new(vec2(width, height));
    let dark = Color32::from_gray(30);
    let mut x = head_w;
    for (i, &c) in threads.iter().enumerate() {
        svg.text(
            pos2(x + PAD, 4.0),
            &format!("#{c}"),
            12.0,
            dark,
            false,
            true,
        );
        x += col_w[i];
    }
    let mut y = LINE + PAD;
    for (ri, &r) in threads.iter().enumerate() {
        svg.text(
            pos2(PAD, y + 4.0),
            &thread_label(program, r),
            12.0,
            dark,
            false,
            true,
        );
        let mut x = head_w;
        for (ci, &c) in threads.iter().enumerate() {
            if r == c {
                let cell = Rect::from_min_size(pos2(x, y), vec2(col_w[ci], row_h[ri]));
                svg.rect(cell, Color32::from_gray(225), None, 0.0);
            }
            for (k, &comm) in cells.get(&(r, c)).into_iter().flatten().enumerate() {
                let (text, color) = chip(program, comm);
                svg.text(
                    pos2(x + PAD, y + 4.0 + k as f32 * LINE),
                    &text,
                    12.0,
                    color,
                    true,
                    false,
                );
            }
            x += col_w[ci];
        }
        y += row_h[ri];
    }
    let gray = Color32::from_gray(200);
    let mut xs = vec![0.0, head_w];
    for w in &col_w {
        xs.push(xs[xs.len() - 1] + w);
    }
    let mut ys = vec![0.0, LINE + PAD];
    for h in &row_h {
        ys.push(ys[ys.len() - 1] + h);
    }
    for &x in &xs {
        svg.polyline(&[pos2(x, 0.0), pos2(x, height)], 1.0, gray, None);
    }
    for &y in &ys {
        svg.polyline(&[pos2(0.0, y), pos2(width, y)], 1.0, gray, None);
    }
    svg.finish()
}
