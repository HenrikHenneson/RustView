//! The communication matrix as data: which thread passes what to which,
//! and how the communication objects are named.

use std::collections::{BTreeMap, BTreeSet};

use egui::Color32;
use rv_analysis::{Program, op_writes as writes};
use rv_ir::{FnId, NodeId, Project};

use crate::style::{CHANNEL, JOIN, SHARED};

/// A channel, shared object or join of a program, by its index there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum CommRef {
    Channel(usize),
    Shared(usize),
    Join(usize),
}

/// Thread × thread matrix of a program (row → column: what flows from the
/// row thread to the column thread).
pub struct Matrix {
    /// (from, to) -> communication objects.
    cells: BTreeMap<(usize, usize), BTreeSet<CommRef>>,
    /// The threads that communicate: the rows and columns.
    pub threads: Vec<usize>,
}

impl Matrix {
    pub fn new(program: &Program) -> Self {
        let mut cells = BTreeMap::new();
        add_channels(&mut cells, program);
        add_shared(&mut cells, program);
        add_joins(&mut cells, program);
        let threads: BTreeSet<usize> = cells.keys().flat_map(|&(a, b)| [a, b]).collect();
        Matrix {
            cells,
            threads: threads.into_iter().collect(),
        }
    }

    /// What flows from thread `from` to thread `to`.
    pub fn cell(&self, from: usize, to: usize) -> impl Iterator<Item = CommRef> {
        self.cells.get(&(from, to)).into_iter().flatten().copied()
    }
}

type Cells = BTreeMap<(usize, usize), BTreeSet<CommRef>>;

/// Channels: from every sending to every receiving thread.
fn add_channels(cells: &mut Cells, program: &Program) {
    for (i, c) in program.channels.iter().enumerate() {
        for s in c.senders.iter().map(|s| s.thread) {
            for r in c.receivers.iter().map(|s| s.thread) {
                if s != r {
                    cells.entry((s, r)).or_default().insert(CommRef::Channel(i));
                }
            }
        }
    }
}

/// Shared state: from every thread that changes it (or may: unknown
/// methods) to every other thread using it.
fn add_shared(cells: &mut Cells, program: &Program) {
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
}

/// Joins: from the thread that ends to every thread waiting for it.
fn add_joins(cells: &mut Cells, program: &Program) {
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
}

/// `#2 worker`
pub fn thread_label(program: &Program, t: usize) -> String {
    format!("#{t} {}", program.threads[t].title())
}

/// `#0, #2`
pub fn thread_set(set: impl IntoIterator<Item = usize>) -> String {
    let v: BTreeSet<usize> = set.into_iter().collect();
    v.iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Name and color of a communication object in a matrix cell.
pub fn chip(program: &Program, c: CommRef) -> (String, Color32) {
    match c {
        CommRef::Channel(i) => (program.channels[i].name.clone(), CHANNEL),
        CommRef::Shared(i) => (program.shared[i].name.clone(), SHARED),
        CommRef::Join(_) => ("join".into(), JOIN),
    }
}

/// Hover text of a chip: what it is and where it comes from.
pub fn chip_tooltip(project: &Project, program: &Program, c: CommRef) -> String {
    let origin = |(f, _): (FnId, NodeId)| project.function(f).qual_path.clone();
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
