//! Text answers to single questions about a project (for the MCP server):
//! what a name is, who calls a function, which threads run it, and the
//! nesting of a function. Every answer carries `file:line` references.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::PathBuf;

use rv_ir::{FnId, FrameRole, NodeId, NodeKind, Project, Span, StructKind};

use crate::report::{base_dir, short};
use crate::{CallGraph, Facts, Program};

/// Lines of an outline before it is cut.
const MAX_OUTLINE: usize = 400;
/// Entries of a list answer before it is cut.
const MAX_ENTRIES: usize = 40;

/// Formats locations relative to the project directory.
struct Loc<'p> {
    project: &'p Project,
    base: PathBuf,
}

impl<'p> Loc<'p> {
    fn new(project: &'p Project) -> Self {
        Loc {
            project,
            base: base_dir(&project.root),
        }
    }

    fn file(&self, span: &Span) -> String {
        let path = &self.project.file(span.file).path;
        path.strip_prefix(&self.base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn at(&self, span: &Span) -> String {
        format!("{}:{}", self.file(span), span.start.line)
    }

    fn func(&self, f: FnId) -> String {
        let func = self.project.function(f);
        format!("`{}` ({})", func.qual_path, self.at(&func.name_span))
    }
}

/// A function by full path or by any `::`-suffix of it (`App::update`,
/// `update`). Several matches are an error that lists them.
pub fn resolve_function(project: &Project, name: &str) -> Result<FnId, String> {
    let name = name.trim().trim_matches('`');
    let suffix = format!("::{name}");
    let matches: Vec<FnId> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path == name || f.qual_path.ends_with(&suffix))
        .map(|(i, _)| FnId(i as u32))
        .collect();
    if let Some(&exact) = matches
        .iter()
        .find(|&&f| project.function(f).qual_path == name)
    {
        return Ok(exact);
    }
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!(
            "No function `{name}`. Use `find` to search for a name, or give the \
             path with its module (`module::Type::method`)."
        )),
        many => {
            let loc = Loc::new(project);
            let mut out = format!(
                "`{name}` is ambiguous ({} functions); use a longer path:\n",
                many.len()
            );
            for &f in many.iter().take(MAX_ENTRIES) {
                let _ = writeln!(out, "- {}", loc.func(f));
            }
            Err(out)
        }
    }
}

/// Everything called `name`: functions, threads, channel ends, shared state,
/// struct fields and types.
pub fn find(project: &Project, programs: &[(FnId, Program)], name: &str) -> String {
    let name = name.trim().trim_matches('`');
    let loc = Loc::new(project);
    let mut out = String::new();
    let last = name.rsplit("::").next().unwrap_or(name);
    let suffix = format!("::{name}");

    let fns: Vec<FnId> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == last && (f.qual_path == name || f.qual_path.ends_with(&suffix)))
        .map(|(i, _)| FnId(i as u32))
        .collect();
    if !fns.is_empty() {
        let _ = writeln!(out, "Functions:");
        for &f in fns.iter().take(MAX_ENTRIES) {
            let func = project.function(f);
            let _ = writeln!(out, "- {}: `{}`", loc.func(f), func.sig.text);
        }
        more(&mut out, fns.len());
    }

    // Methods of a type of that name.
    let methods: Vec<FnId> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| match &f.owner {
            rv_ir::FnOwner::Impl { self_ty, .. } => {
                self_ty.split('<').next().unwrap_or(self_ty).trim() == last
            }
            _ => false,
        })
        .map(|(i, _)| FnId(i as u32))
        .collect();
    if let Some(fields) = project.struct_fields.get(last) {
        let list: Vec<String> = fields.iter().map(|(f, t)| format!("{f}: {t}")).collect();
        let _ = writeln!(out, "Struct `{last}` fields: {}", list.join(", "));
    }
    if !methods.is_empty() {
        let _ = writeln!(out, "Methods of `{last}`:");
        for &f in methods.iter().take(MAX_ENTRIES) {
            let _ = writeln!(out, "- {}", loc.func(f));
        }
        more(&mut out, methods.len());
    }
    if let Some(target) = project.type_aliases.get(last) {
        let _ = writeln!(out, "Type alias `{last}` = `{target}`");
    }

    for (entry, program) in programs {
        let bin = &project.function(*entry).qual_path;
        for (i, t) in program.threads.iter().enumerate() {
            if t.name.as_deref() == Some(name) {
                let at = t.site.map(|(f, n)| node_at(&loc, f, n)).unwrap_or_default();
                let _ = writeln!(out, "Thread #{i} '{name}' of `{bin}`, started at {at}");
            }
        }
        for c in &program.channels {
            if c.name.split(" → ").any(|end| end == name) {
                let _ = writeln!(
                    out,
                    "Channel `{}` (`{}`) of `{bin}`, created at {}; senders: {}; receivers: {}",
                    c.name,
                    c.api,
                    node_at(&loc, c.site.0, c.site.1),
                    thread_list(&c.senders),
                    thread_list(&c.receivers)
                );
            }
        }
        for s in &program.shared {
            if s.name == name || s.kind.contains(name) {
                let _ = writeln!(
                    out,
                    "Shared state `{}` (`{}`) of `{bin}`, created at {}; used by threads {}",
                    s.name,
                    s.kind,
                    node_at(&loc, s.site.0, s.site.1),
                    thread_list(&s.users)
                );
            }
        }
    }
    if out.is_empty() {
        format!(
            "Nothing named `{name}` (functions, types, threads, channels, shared state). \
             Local variables that are not channels or shared state are not indexed."
        )
    } else {
        out
    }
}

fn more(out: &mut String, total: usize) {
    if total > MAX_ENTRIES {
        let _ = writeln!(out, "- … {} more", total - MAX_ENTRIES);
    }
}

fn node_at(loc: &Loc, f: FnId, node: NodeId) -> String {
    let func = loc.project.function(f);
    match &func.body {
        Some(b) if node.index() < b.nodes.len() => loc.at(&b.node(node).span),
        _ => loc.at(&func.name_span),
    }
}

fn thread_list(sites: &[crate::Site]) -> String {
    let set: std::collections::BTreeSet<usize> = sites.iter().map(|s| s.thread).collect();
    if set.is_empty() {
        return "none found".into();
    }
    set.iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Who calls `f`, with the lines of the calls.
pub fn callers(project: &Project, calls: &CallGraph, f: FnId) -> String {
    let loc = Loc::new(project);
    let mut by: BTreeMap<FnId, Vec<u32>> = BTreeMap::new();
    for e in calls.edges.iter().filter(|e| e.callee == f) {
        by.entry(e.caller)
            .or_default()
            .push(call_line(project, e.caller, e.node));
    }
    let mut out = format!("Callers of {}:\n", loc.func(f));
    if by.is_empty() {
        out.push_str(
            "- none found (an entry point, a callback of a framework or trait, called \
             through a function pointer or macro, or unused)\n",
        );
    }
    list_calls(&mut out, &loc, by);
    out
}

/// What `f` calls (project functions only), with the lines of the calls.
pub fn callees(project: &Project, calls: &CallGraph, f: FnId) -> String {
    let loc = Loc::new(project);
    let mut by: BTreeMap<FnId, Vec<u32>> = BTreeMap::new();
    for e in calls.calls_from(f) {
        by.entry(e.callee)
            .or_default()
            .push(call_line(project, f, e.node));
    }
    let file = loc.file(&project.function(f).name_span);
    let mut out = format!(
        "Project functions called by {} (call lines in {file}):\n",
        loc.func(f)
    );
    if by.is_empty() {
        out.push_str("- none (only std/external calls, or calls that could not be resolved)\n");
    }
    list_calls(&mut out, &loc, by);
    out
}

fn call_line(project: &Project, f: FnId, node: NodeId) -> u32 {
    project
        .function(f)
        .body
        .as_ref()
        .filter(|b| node.index() < b.nodes.len())
        .map_or(0, |b| b.node(node).span.start.line)
}

fn list_calls(out: &mut String, loc: &Loc, by: BTreeMap<FnId, Vec<u32>>) {
    let total = by.len();
    for (f, mut lines) in by.into_iter().take(MAX_ENTRIES) {
        lines.sort_unstable();
        lines.dedup();
        let lines: Vec<String> = lines.iter().map(u32::to_string).collect();
        let _ = writeln!(out, "- {} — line {}", loc.func(f), lines.join(", "));
    }
    more(out, total);
}

/// The threads that run `f`, per binary.
pub fn threads_of(project: &Project, programs: &[(FnId, Program)], f: FnId) -> String {
    let loc = Loc::new(project);
    let mut out = format!("Threads running {}:\n", loc.func(f));
    let mut found = false;
    for (entry, program) in programs {
        for (i, t) in program.threads.iter().enumerate() {
            let how = if t.entries.contains(&f) {
                "called directly by the thread"
            } else if t.callbacks.contains(&f) {
                "framework callback on the thread"
            } else if t.runs.contains(&f) {
                "reached through calls"
            } else {
                continue;
            };
            found = true;
            let _ = writeln!(
                out,
                "- `{}`: #{i} {} ({how})",
                project.function(*entry).qual_path,
                t.title()
            );
        }
    }
    if !found {
        out.push_str(
            "- none: not reached from `main` of any binary (library code that the binaries \
             do not call, a trait method called through a trait object, test code, or unused)\n",
        );
    }
    out
}

/// The nesting of `f`: loops, branches, closures, thread lanes and exits,
/// one line each with its line range. With `statements`, also the plain
/// statements.
pub fn outline(project: &Project, facts: &Facts, f: FnId, statements: bool) -> String {
    let loc = Loc::new(project);
    let func = project.function(f);
    let Some(body) = &func.body else {
        return format!("{} has no body.", loc.func(f));
    };
    let mut marks: BTreeMap<NodeId, Vec<String>> = BTreeMap::new();
    for e in &body.exits {
        marks
            .entry(e.from)
            .or_default()
            .push(format!("→ {}", e.kind.symbol()));
    }
    if let Some(c) = facts.concurrency(f) {
        for s in &c.spawns {
            let what = match &s.name {
                Some(n) => format!("‖ starts {} '{n}'", s.kind.label()),
                None => format!("‖ starts {}", s.kind.label()),
            };
            marks.entry(s.node).or_default().push(what.clone());
            for &lane in &s.lanes {
                marks
                    .entry(lane)
                    .or_default()
                    .push("runs concurrently".into());
            }
        }
    }
    let mut o = Outline {
        body,
        marks,
        statements,
        lines: Vec::new(),
    };
    o.node(body.root, 0);
    let mut out = format!(
        "Outline of {}: {} lines, file {}{}\n",
        loc.func(f),
        func.span.line_count(),
        loc.file(&func.span),
        if statements {
            ""
        } else {
            " (structures only; statements with exits or thread starts are kept)"
        }
    );
    let total = o.lines.len();
    for l in o.lines.into_iter().take(MAX_OUTLINE) {
        out.push_str(&l);
        out.push('\n');
    }
    if total > MAX_OUTLINE {
        let _ = writeln!(out, "… {} more lines", total - MAX_OUTLINE);
    }
    out
}

struct Outline<'b> {
    body: &'b rv_ir::Body,
    marks: BTreeMap<NodeId, Vec<String>>,
    statements: bool,
    lines: Vec<String>,
}

impl Outline<'_> {
    /// The pattern of the only case frame among `children`, if there is
    /// exactly one case and no `else`.
    fn single_case(&self, children: &[NodeId]) -> Option<String> {
        let mut cases = children
            .iter()
            .filter_map(|&c| match &self.body.node(c).kind {
                NodeKind::Frame {
                    role: FrameRole::Case { pattern, .. },
                } => Some(Some(pattern.clone())),
                NodeKind::Frame {
                    role: FrameRole::Else,
                } => Some(None),
                _ => None,
            });
        match (cases.next(), cases.next()) {
            (Some(Some(pattern)), None) => Some(pattern),
            _ => None,
        }
    }

    fn push(&mut self, depth: usize, node: NodeId, text: &str) {
        let n = self.body.node(node);
        let range = if n.span.end.line > n.span.start.line {
            format!("{}-{}", n.span.start.line, n.span.end.line)
        } else {
            n.span.start.line.to_string()
        };
        let mut line = format!("{}{range} {}", "  ".repeat(depth), one_line(text));
        if let Some(m) = self.marks.get(&node) {
            let _ = write!(line, "  [{}]", m.join(", "));
        }
        self.lines.push(line);
    }

    fn node(&mut self, id: NodeId, depth: usize) {
        let n = self.body.node(id).clone();
        match &n.kind {
            NodeKind::Structure { kind, .. } => {
                let depth = if matches!(kind, StructKind::Function) {
                    depth
                } else {
                    // A lone `if` shows its condition (cases are only listed
                    // when there are several).
                    let label = match (kind, self.single_case(&n.children)) {
                        (StructKind::If, Some(pattern)) => format!("if {pattern}"),
                        _ => n.label.clone(),
                    };
                    self.push(depth, id, &label);
                    depth + 1
                };
                let cases = n
                    .children
                    .iter()
                    .filter(|&&c| {
                        matches!(
                            self.body.node(c).kind,
                            NodeKind::Frame {
                                role: FrameRole::Case { .. } | FrameRole::Else
                            }
                        )
                    })
                    .count();
                for &c in &n.children {
                    let frame = self.body.node(c);
                    let named = cases > 1
                        && matches!(
                            frame.kind,
                            NodeKind::Frame {
                                role: FrameRole::Case { .. } | FrameRole::Else
                            }
                        );
                    if named {
                        let label = match &frame.kind {
                            NodeKind::Frame {
                                role: FrameRole::Case { pattern, guard },
                            } => match guard {
                                Some(g) => format!("{pattern} if {g} =>"),
                                None => format!("{pattern} =>"),
                            },
                            _ => "else =>".into(),
                        };
                        self.push(depth, c, &label);
                        self.node(c, depth + 1);
                    } else {
                        self.node(c, depth);
                    }
                }
            }
            NodeKind::Frame { .. } => {
                for &c in &n.children {
                    self.node(c, depth);
                }
            }
            NodeKind::Stmt { .. } => {
                let marked = self.marks.contains_key(&id);
                let shown = self.statements || marked;
                if shown {
                    self.push(depth, id, &n.label);
                }
                let inner = if shown { depth + 1 } else { depth };
                for &c in &n.children {
                    self.node(c, inner);
                }
            }
        }
    }
}

/// The first line of a label, at most 100 chars.
fn one_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    if first.chars().count() > 100 {
        format!("{}…", first.chars().take(99).collect::<String>())
    } else {
        first.to_string()
    }
}

/// Short name used in answers (`Type::method`).
pub fn short_name(project: &Project, f: FnId) -> String {
    short(&project.function(f).qual_path)
}
