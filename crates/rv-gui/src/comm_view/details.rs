//! Right panel of the communication view: all communication objects, or
//! the selected channel, shared object or join with every place using it.

use std::collections::{BTreeMap, BTreeSet};

use egui::{RichText, Ui};
use rv_analysis::{ChannelComm, JoinComm, Program, SharedComm, Site, op_meaning as semantics};
use rv_ir::{FnId, NodeId, Project};

use super::matrix::{CommRef, thread_label, thread_set};
use crate::lookup::node_line;
use crate::style::{CHANNEL, JOIN, SHARED};
use crate::threads::ThreadsAction;
use crate::widgets::mono_link;

/// Space above a group of entries.
const GROUP_SPACE: f32 = 6.0;

/// Draws the details of `selected` (or the list of everything); clicking
/// changes `selected` or asks to show a place in the code.
pub(super) fn details_ui(
    ui: &mut Ui,
    project: &Project,
    program: &Program,
    selected: &mut Option<CommRef>,
) -> Option<ThreadsAction> {
    let Some(comm) = *selected else {
        object_list(ui, program, selected);
        return None;
    };
    match comm {
        CommRef::Channel(i) => {
            let c = &program.channels[i];
            back_header(ui, selected, RichText::new(&c.name).color(CHANNEL));
            channel_details(ui, project, program, c)
        }
        CommRef::Shared(i) => {
            let s = &program.shared[i];
            let title = format!("{} ({})", s.name, s.kind);
            back_header(ui, selected, RichText::new(title).color(SHARED));
            shared_details(ui, project, program, s)
        }
        CommRef::Join(i) => {
            let j = &program.joins[i];
            let title = format!("join {}", program.threads[j.thread].title());
            back_header(ui, selected, RichText::new(title).color(JOIN));
            join_details(ui, project, program, j)
        }
    }
}

/// All channels, shared objects and joins as links that select them.
fn object_list(ui: &mut Ui, program: &Program, selected: &mut Option<CommRef>) {
    ui.heading("Communication objects");
    ui.weak("Click an entry here or in the matrix for details.");
    let channels = program.channels.iter().map(|c| {
        let from: BTreeSet<usize> = c.senders.iter().map(|s| s.thread).collect();
        let to: BTreeSet<usize> = c.receivers.iter().map(|s| s.thread).collect();
        let text = format!("{}  {} → {}", c.name, thread_set(from), thread_set(to));
        RichText::new(text).color(CHANNEL).monospace()
    });
    if let Some(i) = link_group(ui, "Channels", channels.collect()) {
        *selected = Some(CommRef::Channel(i));
    }
    let shared = program.shared.iter().map(|s| {
        let text = format!("{} ({})  {}", s.name, s.kind, thread_set(s.threads()));
        RichText::new(text).color(SHARED).monospace()
    });
    if let Some(i) = link_group(ui, "Shared state", shared.collect()) {
        *selected = Some(CommRef::Shared(i));
    }
    let joins = program.joins.iter().map(|j| {
        let text = format!("waits for {}", thread_label(program, j.thread));
        RichText::new(text).color(JOIN)
    });
    if let Some(i) = link_group(ui, "Joins", joins.collect()) {
        *selected = Some(CommRef::Join(i));
    }
}

/// A titled group of links (nothing if there are none); returns the index
/// of the link that was clicked.
fn link_group(ui: &mut Ui, title: &str, links: Vec<RichText>) -> Option<usize> {
    if links.is_empty() {
        return None;
    }
    ui.add_space(GROUP_SPACE);
    ui.strong(format!("{title} ({})", links.len()));
    let mut clicked = None;
    for (i, link) in links.into_iter().enumerate() {
        if ui.link(link).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// Heading of a selected object with a button back to the list.
fn back_header(ui: &mut Ui, selected: &mut Option<CommRef>, title: RichText) {
    ui.horizontal(|ui| {
        if ui.small_button("← all").clicked() {
            *selected = None;
        }
        ui.heading(title);
    });
}

fn channel_details(
    ui: &mut Ui,
    project: &Project,
    program: &Program,
    c: &ChannelComm,
) -> Option<ThreadsAction> {
    ui.monospace(&c.api);
    let created = site_link(ui, project, c.site);
    ui.label(format!(
        "message type: {}",
        c.message.as_deref().unwrap_or("(not written in the code)")
    ));
    let bounded = c.capacity.is_some();
    ui.label(match &c.capacity {
        Some(n) => format!("bounded: {n} messages"),
        None => "unbounded (or capacity not visible)".into(),
    });
    ui.add_space(GROUP_SPACE);
    ui.strong("sends");
    let sends = site_list(ui, project, program, &c.senders, bounded);
    ui.add_space(GROUP_SPACE);
    ui.strong("receives");
    let receives = site_list(ui, project, program, &c.receivers, bounded);
    created.or(sends).or(receives)
}

fn shared_details(
    ui: &mut Ui,
    project: &Project,
    program: &Program,
    s: &SharedComm,
) -> Option<ThreadsAction> {
    let created = site_link(ui, project, s.site);
    ui.add_space(GROUP_SPACE);
    ui.strong("used by");
    let used = site_list(ui, project, program, &s.users, false);
    created.or(used)
}

fn join_details(
    ui: &mut Ui,
    project: &Project,
    program: &Program,
    j: &JoinComm,
) -> Option<ThreadsAction> {
    ui.strong("waiting");
    site_list(ui, project, program, &j.waiters, false)
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
                if site_row(ui, project, s, bounded) {
                    action = Some(ThreadsAction::ShowNode(s.func, s.node));
                }
            }
        });
    }
    action
}

/// One place: function and line as a link, the operation and what it does.
/// True when the link was clicked.
fn site_row(ui: &mut Ui, project: &Project, s: &Site, bounded: bool) -> bool {
    let func = project.function(s.func);
    let line = node_line(project, s.func, s.node);
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        let link = mono_link(ui, format!("{}:{line}", func.name));
        clicked = link.on_hover_text(&func.qual_path).clicked();
        if let Some(op) = &s.op {
            ui.monospace(op);
        }
        let long = semantics(s.op.as_deref(), bounded);
        match rv_analysis::op_effect(s.op.as_deref(), bounded) {
            Some(effect) => {
                ui.weak(effect).on_hover_text(long);
            }
            None => {
                ui.weak(long);
            }
        }
    });
    clicked
}

/// "created in …" as a link to that place.
fn site_link(ui: &mut Ui, project: &Project, (f, node): (FnId, NodeId)) -> Option<ThreadsAction> {
    let line = node_line(project, f, node);
    ui.link(format!(
        "created in {}:{line}",
        project.function(f).qual_path
    ))
    .clicked()
    .then_some(ThreadsAction::ShowNode(f, node))
}
