//! Text answers to single questions about a project (for the MCP server):
//! what a name is, who calls a function, which threads run it, and the
//! nesting of a function. Every answer carries `file:line` references.

use std::collections::BTreeMap;
use std::fmt::Write;

use rv_ir::{FnId, NodeId, Project};

use crate::locate::{Loc, short};
use crate::{CallGraph, Program};

mod outline;

pub use outline::outline;

/// Entries of a list answer before it is cut.
const MAX_ENTRIES: usize = 40;

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
    find_functions(&mut out, &loc, name, last);
    find_type(&mut out, &loc, last);
    for (entry, program) in programs {
        let bin = &project.function(*entry).qual_path;
        find_in_program(&mut out, &loc, program, bin, name);
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

/// Functions called `name` (`last` is its last path segment).
fn find_functions(out: &mut String, loc: &Loc, name: &str, last: &str) {
    let project = loc.project;
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
        more(out, fns.len());
    }
}

/// The type called `name`: its fields, its methods and what it is an alias of.
fn find_type(out: &mut String, loc: &Loc, name: &str) {
    let project = loc.project;
    let methods: Vec<FnId> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| match &f.owner {
            rv_ir::FnOwner::Impl { self_ty, .. } => {
                self_ty.split('<').next().unwrap_or(self_ty).trim() == name
            }
            _ => false,
        })
        .map(|(i, _)| FnId(i as u32))
        .collect();
    if let Some(fields) = project.struct_fields.get(name) {
        let list: Vec<String> = fields.iter().map(|(f, t)| format!("{f}: {t}")).collect();
        let _ = writeln!(out, "Struct `{name}` fields: {}", list.join(", "));
    }
    if !methods.is_empty() {
        let _ = writeln!(out, "Methods of `{name}`:");
        for &f in methods.iter().take(MAX_ENTRIES) {
            let _ = writeln!(out, "- {}", loc.func(f));
        }
        more(out, methods.len());
    }
    if let Some(target) = project.type_aliases.get(name) {
        let _ = writeln!(out, "Type alias `{name}` = `{target}`");
    }
}

/// Threads, channels and shared state called `name` in the binary `bin`.
fn find_in_program(out: &mut String, loc: &Loc, program: &Program, bin: &str, name: &str) {
    for (i, t) in program.threads.iter().enumerate() {
        if t.name.as_deref() == Some(name) {
            let at = t.site.map(|(f, n)| loc.node(f, n)).unwrap_or_default();
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
                loc.node(c.site.0, c.site.1),
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
                loc.node(s.site.0, s.site.1),
                thread_list(&s.users)
            );
        }
    }
}

fn more(out: &mut String, total: usize) {
    if total > MAX_ENTRIES {
        let _ = writeln!(out, "- … {} more", total - MAX_ENTRIES);
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

/// Short name used in answers (`Type::method`).
pub fn short_name(project: &Project, f: FnId) -> String {
    short(&project.function(f).qual_path)
}
