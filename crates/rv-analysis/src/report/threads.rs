//! The threads of a binary: who starts them, what they run and how they
//! communicate.

use std::collections::BTreeMap;
use std::fmt::Write;

use rv_ir::{FnId, Project};

use super::writer::{MAX_LIST, Report, plural, thread_ref};
use crate::locate::short;
use crate::{Program, Thread};

/// Entry functions and modules named per thread.
const MAX_NAMED: usize = 4;
/// Framework callbacks named per thread.
const MAX_CALLBACKS: usize = 6;

impl Report<'_> {
    pub(super) fn threads(&mut self, entry: FnId, program: &Program) {
        let p = self.project;
        let bin = &p.function(entry).qual_path;
        self.line(format!("## Threads of `{bin}`"));
        self.line("");
        if program.threads.len() <= 1 && program.unreached.is_empty() {
            self.line("No threads or tasks are started on the way from `main`.");
            self.line("");
            return;
        }
        self.line(
            "Thread #0 is the main thread. A thread runs the functions reachable from \
             its entry without passing another thread start. The effect after a site \
             (blocking, non-blocking, may drop, atomic) follows from the API method \
             and the channel capacity, not from the code around the call.",
        );
        self.line("");
        for i in 0..program.threads.len() {
            self.thread(program, i);
        }
        self.line("");
        self.channels(program);
        self.shared(program);
        self.joins(program);
        self.unreached_starts(program);
    }

    /// One line about thread `i`: where it starts and what it runs.
    fn thread(&mut self, program: &Program, i: usize) {
        let p = self.project;
        let t = &program.threads[i];
        let mut text = format!("- **{}**", thread_ref(program, i));
        if let Some(name) = &t.name {
            self.keyword("thread names", format!("\"{name}\""));
        }
        if let Some((f, node)) = t.site {
            let by: Vec<String> = t.spawned_by.iter().map(|b| format!("#{b}")).collect();
            let _ = write!(
                text,
                " — started by {} at {} in `{}`",
                by.join("/"),
                self.loc.node(f, node),
                p.function(f).qual_path
            );
            self.keyword(
                "functions that start threads",
                short(&p.function(f).qual_path),
            );
        }
        let entries: Vec<String> = t
            .entries
            .iter()
            .take(MAX_NAMED)
            .map(|&f| self.loc.func(f))
            .collect();
        if !entries.is_empty() {
            let _ = write!(text, "; calls {}", entries.join(", "));
            if t.entries.len() > MAX_NAMED {
                let _ = write!(text, " +{}", t.entries.len() - MAX_NAMED);
            }
        }
        text.push_str(&runs(p, t));
        text.push_str(&callbacks(p, t));
        self.line(text);
    }

    /// Spawn calls the walk from `main` did not reach.
    fn unreached_starts(&mut self, program: &Program) {
        if program.unreached.is_empty() {
            return;
        }
        let p = self.project;
        self.unreached += program.unreached.len();
        self.line("### Thread starts not reached from `main`");
        self.line("");
        self.line(
            "Spawn calls in functions the call graph walk did not reach (called \
             through traits, function pointers, macros or not at all):",
        );
        self.line("");
        for &(f, node) in program.unreached.iter().take(MAX_LIST) {
            self.line(format!(
                "- {} in `{}`",
                self.loc.node(f, node),
                p.function(f).qual_path
            ));
        }
        self.more(program.unreached.len(), MAX_LIST);
        self.line("");
    }
}

/// Module path cut to `crate::top_module`.
fn area(project: &Project, f: FnId) -> String {
    let path = &project.module(project.function(f).module).path;
    path[..path.len().min(2)].join("::")
}

/// `; runs 12 functions (by module: …)`: how many functions the thread runs
/// and the modules that hold most of them. Empty if it runs none.
fn runs(project: &Project, t: &Thread) -> String {
    let mut areas: BTreeMap<String, usize> = BTreeMap::new();
    for &f in &t.runs {
        *areas.entry(area(project, f)).or_default() += 1;
    }
    let mut areas: Vec<(String, usize)> = areas.into_iter().collect();
    areas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let areas: Vec<String> = areas
        .iter()
        .take(MAX_NAMED)
        .map(|(a, n)| format!("`{a}` {n}"))
        .collect();
    if areas.is_empty() {
        return String::new();
    }
    format!(
        "; runs {} function{} (by module: {})",
        t.runs.len(),
        plural(t.runs.len()),
        areas.join(", ")
    )
}

/// `; framework callbacks …`: the trait methods a framework calls on the
/// thread, each name once. Empty if there are none.
fn callbacks(project: &Project, t: &Thread) -> String {
    if t.callbacks.is_empty() {
        return String::new();
    }
    let mut cbs: Vec<String> = Vec::new();
    for &f in &t.callbacks {
        let name = format!("`{}`", short(&project.function(f).qual_path));
        if !cbs.contains(&name) {
            cbs.push(name);
        }
    }
    let more = cbs.len().saturating_sub(MAX_CALLBACKS);
    cbs.truncate(MAX_CALLBACKS);
    let mut text = format!("; framework callbacks {}", cbs.join(", "));
    if more > 0 {
        let _ = write!(text, " +{more}");
    }
    text
}
