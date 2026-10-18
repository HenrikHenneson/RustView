//! A compact Markdown summary of a project for readers who do not look at the
//! diagrams — mainly LLM assistants starting to work on unfamiliar code. Every
//! statement carries a `file:line` and the names to search for.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use rv_ir::{Body, FnId, NodeId, NodeKind, Project, Severity, Span};

use crate::{
    CallGraph, Facts, Program, Site, crates, entry_points, module_graph, op_effect, op_writes,
    program,
};

/// Sites listed per channel or shared object.
const MAX_SITES: usize = 8;
/// Modules listed per crate.
const MAX_MODULES: usize = 16;
/// Entries of the other lists (functions, modules, unreached starts, ...).
const MAX_LIST: usize = 12;

/// The report for the whole project.
pub fn report(project: &Project) -> String {
    let facts = Facts::new(project);
    let calls = CallGraph::new(project);
    let mut r = Report {
        project,
        base: base_dir(&project.root),
        out: String::new(),
        keywords: Vec::new(),
        not_followed: 0,
        unreached: 0,
    };
    r.header();
    r.overview();
    for entry in entry_points(project) {
        if project
            .function(entry)
            .qual_path
            .starts_with("build_script")
        {
            continue;
        }
        let program = program(project, &facts, &calls, entry);
        r.threads(entry, &program);
    }
    r.modules(&facts, &calls);
    r.hot_spots(&calls);
    r.keywords();
    r.limits();
    r.out
}

struct Report<'p> {
    project: &'p Project,
    /// Paths are shown relative to this directory.
    base: PathBuf,
    out: String,
    /// (category, names) in order of appearance.
    keywords: Vec<(&'static str, Vec<String>)>,
    /// Sites where a value is handed on in a way the analysis does not follow.
    not_followed: usize,
    unreached: usize,
}

fn base_dir(root: &Path) -> PathBuf {
    if root.is_file() {
        root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        root.to_path_buf()
    }
}

/// Last two path segments: `Zyklus::fahren`, `task::run`.
fn short(qual: &str) -> String {
    let parts: Vec<&str> = qual.rsplitn(3, "::").collect();
    match parts.as_slice() {
        [last, prev, ..] => format!("{prev}::{last}"),
        _ => qual.to_string(),
    }
}

/// Nesting depth of structures in a body (the function itself is 0).
fn depth(body: &Body) -> usize {
    let is_structure = |n: NodeId| matches!(body.node(n).kind, NodeKind::Structure { .. });
    (0..body.nodes.len())
        .map(|i| NodeId(i as u32))
        .filter(|&n| is_structure(n))
        .map(|n| {
            let mut d = 0;
            let mut cur = body.node(n).parent;
            while let Some(p) = cur {
                if is_structure(p) {
                    d += 1;
                }
                cur = body.node(p).parent;
            }
            d
        })
        .max()
        .unwrap_or(0)
}

/// Module path cut to `crate::top_module`.
fn area(project: &Project, f: FnId) -> String {
    let path = &project.module(project.function(f).module).path;
    path[..path.len().min(2)].join("::")
}

impl Report<'_> {
    fn line(&mut self, text: impl AsRef<str>) {
        self.out.push_str(text.as_ref());
        self.out.push('\n');
    }

    fn keyword(&mut self, category: &'static str, name: impl Into<String>) {
        let name = name.into();
        if name.is_empty() {
            return;
        }
        match self.keywords.iter_mut().find(|(c, _)| *c == category) {
            Some((_, names)) => {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => self.keywords.push((category, vec![name])),
        }
    }

    fn path(&self, path: &Path) -> String {
        path.strip_prefix(&self.base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn loc(&self, span: &Span) -> String {
        format!(
            "{}:{}",
            self.path(&self.project.file(span.file).path),
            span.start.line
        )
    }

    /// `` `a::b::f` (src/b.rs:12) ``
    fn func(&self, f: FnId) -> String {
        let func = self.project.function(f);
        format!("`{}` ({})", func.qual_path, self.loc(&func.name_span))
    }

    /// Location of a node, falling back to the function.
    fn node_loc(&self, f: FnId, node: NodeId) -> String {
        let func = self.project.function(f);
        match &func.body {
            Some(body) if node.index() < body.nodes.len() => self.loc(&body.node(node).span),
            _ => self.loc(&func.name_span),
        }
    }

    fn header(&mut self) {
        let name = self
            .base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.project.root.display().to_string());
        self.line(format!("# RustView report: {name}"));
        self.line("");
        self.line(format!(
            "Generated by RustView {} — syntactic analysis: macros are not expanded, \
             names and types are resolved by heuristics. Treat it as a map and \
             check the code at the given `file:line` before relying on a detail. \
             Paths are relative to the project directory.",
            env!("CARGO_PKG_VERSION")
        ));
        self.line("");
    }

    fn overview(&mut self) {
        let p = self.project;
        self.line("## Overview");
        self.line("");
        for pkg in &p.packages {
            let targets: Vec<String> = pkg
                .targets
                .iter()
                .map(|t| format!("{} `{}`", t.kind.label(), t.name))
                .collect();
            let outside = if pkg.in_workspace {
                ""
            } else {
                " (not a workspace member)"
            };
            self.line(format!(
                "- package `{}`{outside}: {} — {}",
                pkg.name,
                targets.join(", "),
                self.path(&pkg.manifest_path)
            ));
        }
        let lines: usize = p.files.iter().map(|f| f.text.lines().count()).sum();
        let with_body = p.functions.iter().filter(|f| f.body.is_some()).count();
        let asyncs = p.functions.iter().filter(|f| f.sig.is_async).count();
        let unsafes = p.functions.iter().filter(|f| f.sig.is_unsafe).count();
        self.line(format!(
            "- size: {} file{}, {lines} lines, {with_body} functions with body \
             ({asyncs} async, {unsafes} unsafe)",
            p.files.len(),
            plural(p.files.len())
        ));
        let mut tests = rv_ir::TestSummary::default();
        for pkg in &p.packages {
            tests.test_fns += pkg.tests.test_fns;
            tests.cfg_test_modules += pkg.tests.cfg_test_modules;
            tests.test_targets += pkg.tests.test_targets;
            tests.bench_targets += pkg.tests.bench_targets;
            tests.example_targets += pkg.tests.example_targets;
        }
        self.line(format!(
            "- tests (not analyzed): {} #[test] functions, {} cfg(test) modules, \
             {} test / {} bench / {} example targets",
            tests.test_fns,
            tests.cfg_test_modules,
            tests.test_targets,
            tests.bench_targets,
            tests.example_targets
        ));
        let entries: Vec<FnId> = entry_points(p)
            .into_iter()
            .filter(|&f| !p.function(f).qual_path.starts_with("build_script"))
            .collect();
        if entries.is_empty() {
            self.line("- entry points: none (library only)");
        } else {
            let list: Vec<String> = entries.iter().map(|&f| self.func(f)).collect();
            self.line(format!("- entry points: {}", list.join(", ")));
        }
        let problems: Vec<_> = p
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .collect();
        if !problems.is_empty() {
            self.line(format!("- loading problems: {}", problems.len()));
            for d in problems.iter().take(5) {
                let at = match (&d.span, &d.file) {
                    (Some(s), _) => format!("{}: ", self.loc(s)),
                    (None, Some(f)) => format!("{}: ", self.path(f)),
                    _ => String::new(),
                };
                self.line(format!("  - {at}{}", d.message));
            }
        }
        self.line("");
    }

    fn thread_ref(program: &Program, t: usize) -> String {
        format!("#{t} {}", program.threads[t].title())
    }

    fn threads(&mut self, entry: FnId, program: &Program) {
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
             its entry without passing another thread start.",
        );
        self.line("");
        for (i, t) in program.threads.iter().enumerate() {
            let mut text = format!("- **{}**", Self::thread_ref(program, i));
            if let Some(name) = &t.name {
                self.keyword("thread names", format!("\"{name}\""));
            }
            if let Some((f, node)) = t.site {
                let by: Vec<String> = t.spawned_by.iter().map(|b| format!("#{b}")).collect();
                let _ = write!(
                    text,
                    " — started by {} at {} in `{}`",
                    by.join("/"),
                    self.node_loc(f, node),
                    p.function(f).qual_path
                );
                self.keyword(
                    "functions that start threads",
                    short(&p.function(f).qual_path),
                );
            }
            let entries: Vec<String> = t.entries.iter().take(4).map(|&f| self.func(f)).collect();
            if !entries.is_empty() {
                let _ = write!(text, "; calls {}", entries.join(", "));
                if t.entries.len() > 4 {
                    let _ = write!(text, " +{}", t.entries.len() - 4);
                }
            }
            let mut areas: BTreeMap<String, usize> = BTreeMap::new();
            for &f in &t.runs {
                *areas.entry(area(p, f)).or_default() += 1;
            }
            let mut areas: Vec<(String, usize)> = areas.into_iter().collect();
            areas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let areas: Vec<String> = areas
                .iter()
                .take(4)
                .map(|(a, n)| format!("`{a}` {n}"))
                .collect();
            if !areas.is_empty() {
                let _ = write!(
                    text,
                    "; runs {} function{} (by module: {})",
                    t.runs.len(),
                    plural(t.runs.len()),
                    areas.join(", ")
                );
            }
            if !t.callbacks.is_empty() {
                let mut cbs: Vec<String> = Vec::new();
                for &f in &t.callbacks {
                    let name = format!("`{}`", short(&p.function(f).qual_path));
                    if !cbs.contains(&name) {
                        cbs.push(name);
                    }
                }
                let more = cbs.len().saturating_sub(6);
                cbs.truncate(6);
                let _ = write!(text, "; framework callbacks {}", cbs.join(", "));
                if more > 0 {
                    let _ = write!(text, " +{more}");
                }
            }
            self.line(text);
        }
        self.line("");
        self.channels(program);
        self.shared(program);
        self.joins(program);
        if !program.unreached.is_empty() {
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
                    self.node_loc(f, node),
                    p.function(f).qual_path
                ));
            }
            self.more(program.unreached.len(), MAX_LIST);
            self.line("");
        }
    }

    fn more(&mut self, total: usize, shown: usize) {
        if total > shown {
            self.line(format!("- … {} more", total - shown));
        }
    }

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
            let file = self.path(
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
                Self::thread_ref(program, *thread),
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

    fn channels(&mut self, program: &Program) {
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
                self.node_loc(c.site.0, c.site.1),
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

    fn shared(&mut self, program: &Program) {
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
                self.node_loc(sh.site.0, sh.site.1),
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

    fn joins(&mut self, program: &Program) {
        if program.joins.is_empty() {
            return;
        }
        self.line("### Waiting for threads to finish");
        self.line("");
        for j in &program.joins {
            for w in j.waiters.iter().take(MAX_SITES) {
                self.line(format!(
                    "- {} waits for {} at {} in `{}`",
                    Self::thread_ref(program, w.thread),
                    Self::thread_ref(program, j.thread),
                    self.node_loc(w.func, w.node),
                    short(&self.project.function(w.func).qual_path)
                ));
            }
        }
        self.line("");
    }

    fn modules(&mut self, facts: &Facts, calls: &CallGraph) {
        let p = self.project;
        let mut explained = false;
        for crate_name in crates(p) {
            if crate_name.starts_with("build_script") {
                continue;
            }
            let g = module_graph(p, calls, facts, &crate_name, 2);
            if g.nodes.is_empty() {
                continue;
            }
            self.line(format!("## Modules of `{crate_name}`"));
            self.line("");
            if !std::mem::replace(&mut explained, true) {
                self.line("Top-level modules with size and the modules they call (×call sites).");
                self.line("");
            }
            let mut order: Vec<usize> = (0..g.nodes.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(g.nodes[i].lines));
            for &i in order.iter().take(MAX_MODULES) {
                let n = &g.nodes[i];
                let file = n
                    .modules
                    .first()
                    .map(|&m| self.path(&p.file(p.module(m).file).path))
                    .unwrap_or_default();
                let mut text = format!(
                    "- `{}` ({file}): {} lines, {} functions",
                    n.key,
                    n.lines,
                    n.functions.len()
                );
                if n.spawns > 0 {
                    let _ = write!(text, ", {} thread start{}", n.spawns, plural(n.spawns));
                }
                let mut out: Vec<(usize, usize)> = g
                    .edges
                    .iter()
                    .filter(|e| e.from == i)
                    .map(|e| (e.to, e.calls.len()))
                    .collect();
                out.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
                if !out.is_empty() {
                    let list: Vec<String> = out
                        .iter()
                        .take(6)
                        .map(|&(to, n)| format!("`{}` ×{n}", g.nodes[to].key))
                        .collect();
                    let _ = write!(text, " → {}", list.join(", "));
                    if out.len() > 6 {
                        let _ = write!(text, " +{}", out.len() - 6);
                    }
                }
                self.line(text);
            }
            self.more(g.nodes.len(), MAX_MODULES);
            self.line("");
        }
    }

    fn hot_spots(&mut self, calls: &CallGraph) {
        let p = self.project;
        let mut sized: Vec<(FnId, u32, usize)> = p
            .functions
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let body = f.body.as_ref()?;
                Some((FnId(i as u32), f.span.line_count(), depth(body)))
            })
            .collect();
        if sized.is_empty() {
            return;
        }
        self.line("## Hot spots");
        self.line("");
        self.line("Largest functions (lines, deepest nesting of loops/branches/closures):");
        self.line("");
        sized.sort_by_key(|&(f, lines, _)| (std::cmp::Reverse(lines), f));
        for &(f, lines, d) in sized.iter().take(MAX_LIST) {
            self.keyword("largest functions", short(&p.function(f).qual_path));
            self.line(format!("- {}: {lines} lines, depth {d}", self.func(f)));
        }
        self.line("");
        let mut callers: HashMap<FnId, BTreeSet<FnId>> = HashMap::new();
        for e in &calls.edges {
            if e.caller != e.callee {
                callers.entry(e.callee).or_default().insert(e.caller);
            }
        }
        let mut used: Vec<(FnId, usize)> = callers.into_iter().map(|(f, c)| (f, c.len())).collect();
        used.sort_by_key(|&(f, n)| (std::cmp::Reverse(n), f));
        if used.first().is_some_and(|&(_, n)| n > 1) {
            self.line("Most called functions (distinct callers found in the project):");
            self.line("");
            for &(f, n) in used.iter().take(MAX_LIST).filter(|&&(_, n)| n > 1) {
                self.keyword("most called functions", short(&p.function(f).qual_path));
                self.line(format!("- {}: {n} callers", self.func(f)));
            }
            self.line("");
        }
    }

    fn keywords(&mut self) {
        if self.keywords.is_empty() {
            return;
        }
        self.line("## Names to search for");
        self.line("");
        for (category, names) in std::mem::take(&mut self.keywords) {
            let list: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
            self.line(format!("- {category}: {}", list.join(", ")));
        }
        self.line("");
    }

    fn limits(&mut self) {
        self.line("## Limits of this report");
        self.line("");
        self.line(
            "- Threads, channels and shared state are recognized by API names \
             (std, tokio, smol, async-std, crossbeam, flume, rayon, ...); custom \
             wrappers are only seen through the calls inside them.",
        );
        self.line(
            "- Method calls are resolved by name and by the types written in struct \
             fields and signatures; calls through trait objects, generics or \
             function pointers can be missing.",
        );
        self.line("- `cfg` attributes are not evaluated; test code is only counted.");
        if self.not_followed > 0 {
            self.line(format!(
                "- {} sites above are marked `?`: the value is handed on in a way \
                 the analysis does not follow.",
                self.not_followed
            ));
        }
        if self.unreached > 0 {
            self.line(format!(
                "- {} thread starts were not reached from `main`; their threads \
                 are missing from the thread lists.",
                self.unreached
            ));
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
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
