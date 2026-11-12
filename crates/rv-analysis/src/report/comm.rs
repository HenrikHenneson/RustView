//! How the threads of a binary communicate: channels, shared state and
//! joins, each with the sites where the threads use them.

use std::collections::BTreeSet;
use std::fmt::Write;

use rv_ir::{FnId, NodeId};

use super::writer::{Report, thread_ref};
use crate::locate::short;
use crate::{Program, Site, op_effect, op_writes};

/// Sites listed per channel or shared object.
const MAX_SITES: usize = 8;

impl Report<'_> {
    /// Sites grouped by role, thread, function and operation, one line each:
    /// `send in #0 main thread: `send` in `App::run` (src/app.rs:12, 40) — ...`.
    fn sites(&mut self, program: &Program, sites: &[(&Site, &str)], bounded: bool) {
        /// (role, thread, function, operation)
        type Key<'a> = (&'a str, usize, FnId, Option<&'a str>);
        let mut groups: Vec<(Key, Vec<NodeId>)> = Vec::new();
        for &(s, role) in sites {
            if s.op.is_none() {
                self.not_followed += 1;
            }
            let key = (role, s.thread, s.func, s.op.as_deref());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, nodes)) => nodes.push(s.node),
                None => groups.push((key, vec![s.node])),
            }
        }
        for ((role, thread, f, op), nodes) in groups.iter().take(MAX_SITES) {
            let mut lines: Vec<u32> = nodes
                .iter()
                .filter_map(|&n| {
                    let body = self.project.function(*f).body.as_ref()?;
                    (n.index() < body.nodes.len()).then(|| body.node(n).span.start.line)
                })
                .collect();
            lines.sort_unstable();
            lines.dedup();
            let file = self.loc.path(
                &self
                    .project
                    .file(self.project.function(*f).name_span.file)
                    .path,
            );
            let lines: Vec<String> = lines.iter().map(u32::to_string).collect();
            let meaning = op_effect(*op, bounded)
                .map(|e| format!(" — {e}"))
                .unwrap_or_default();
            self.line(format!(
                "  - {role} in {}: `{}` in `{}` ({file}:{}){meaning}",
                thread_ref(program, *thread),
                op.unwrap_or("?"),
                short(&self.project.function(*f).qual_path),
                lines.join(", "),
            ));
        }
        if groups.len() > MAX_SITES {
            let rest: usize = groups[MAX_SITES..].iter().map(|(_, n)| n.len()).sum();
            self.line(format!(
                "  - … {rest} more sites in {} more places",
                groups.len() - MAX_SITES
            ));
        }
    }

    pub(super) fn channels(&mut self, program: &Program) {
        if program.channels.is_empty() {
            return;
        }
        self.line("### Channels");
        self.line("");
        for c in &program.channels {
            for end in c.name.split(" → ") {
                self.keyword("channel ends", end);
            }
            if let Some(m) = &c.message {
                self.keyword("message types", m.clone());
            }
            let from: BTreeSet<usize> = c.senders.iter().map(|s| s.thread).collect();
            let to: BTreeSet<usize> = c.receivers.iter().map(|s| s.thread).collect();
            let arrow = format!(
                "{} → {}",
                ids(&from).unwrap_or_else(|| "?".into()),
                ids(&to).unwrap_or_else(|| "?".into())
            );
            let mut text = format!("- **`{}`** {arrow}: `{}`", c.name, c.api);
            if let Some(m) = &c.message {
                let _ = write!(text, ", message `{m}`");
            }
            match &c.capacity {
                Some(cap) => {
                    let _ = write!(text, ", capacity {cap}");
                }
                None => text.push_str(", unbounded"),
            }
            let _ = write!(
                text,
                ", created at {} in `{}`",
                self.loc.node(c.site.0, c.site.1),
                short(&self.project.function(c.site.0).qual_path)
            );
            self.line(text);
            let bounded = c.capacity.is_some();
            let sites: Vec<(&Site, &str)> = c
                .senders
                .iter()
                .map(|s| (s, "send"))
                .chain(c.receivers.iter().map(|s| (s, "recv")))
                .collect();
            self.sites(program, &sites, bounded);
        }
        self.line("");
    }

    pub(super) fn shared(&mut self, program: &Program) {
        if program.shared.is_empty() {
            return;
        }
        self.line("### Shared state");
        self.line("");
        for sh in &program.shared {
            self.keyword("shared state", sh.name.clone());
            self.keyword("shared types", sh.kind.clone());
            let writers: BTreeSet<usize> = sh
                .users
                .iter()
                .filter(|s| s.op.as_deref().is_some_and(op_writes))
                .map(|s| s.thread)
                .collect();
            let mut text = format!(
                "- **`{}`** `{}`: used by {}",
                sh.name,
                sh.kind,
                ids(&sh.threads()).unwrap_or_else(|| "?".into())
            );
            if let Some(w) = ids(&writers) {
                let _ = write!(text, ", changed by {w}");
            }
            let _ = write!(
                text,
                "; created at {} in `{}`",
                self.loc.node(sh.site.0, sh.site.1),
                short(&self.project.function(sh.site.0).qual_path)
            );
            self.line(text);
            // Changing sites first: they matter most for races and lock order.
            let mut users: Vec<&Site> = sh.users.iter().collect();
            users.sort_by_key(|s| !s.op.as_deref().is_some_and(op_writes));
            let users: Vec<(&Site, &str)> = users.into_iter().map(|s| (s, "use")).collect();
            self.sites(program, &users, false);
        }
        self.line("");
    }

    pub(super) fn joins(&mut self, program: &Program) {
        if program.joins.is_empty() {
            return;
        }
        self.line("### Waiting for threads to finish");
        self.line("");
        for j in &program.joins {
            for w in j.waiters.iter().take(MAX_SITES) {
                self.line(format!(
                    "- {} waits for {} at {} in `{}`",
                    thread_ref(program, w.thread),
                    thread_ref(program, j.thread),
                    self.loc.node(w.func, w.node),
                    short(&self.project.function(w.func).qual_path)
                ));
            }
        }
        self.line("");
    }
}

/// `#1, #2` or `None` for an empty set.
fn ids(set: &BTreeSet<usize>) -> Option<String> {
    if set.is_empty() {
        return None;
    }
    Some(
        set.iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}
