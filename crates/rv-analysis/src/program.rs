//! Whole-program view: a syntactic call graph, which thread runs which code
//! (starting at an entry point such as `main`), and how threads are
//! connected by channels and shared state across function boundaries.
//!
//! Approximations (purely syntactic):
//! - Calls are resolved by path (module/crate aware) and methods by `self`
//!   or by a unique name; ambiguous calls are left out instead of guessed.
//! - Trait impls of a type whose constructor a thread reaches are treated as
//!   callbacks run by that thread (e.g. `impl eframe::App for App`).
//! - Channel ends and shared handles are followed through clones and call
//!   arguments into parameters; storing them in struct fields stops the trail
//!   and attributes the use to the current thread.

use rv_ir::{FnId, FnOwner, Project};

mod callgraph;
mod connect;
mod facts;
mod follow;
mod model;
mod resolver;
mod walker;

pub use callgraph::{CallEdge, CallGraph};
pub use facts::Facts;
pub use model::{
    ChannelComm, JoinComm, Link, LinkKind, Program, Resource, SharedComm, Site, Thread, op_effect,
    op_meaning, op_writes,
};

/// `main` functions at the root of a target (one per binary).
/// Build scripts come last.
pub fn entry_points(project: &Project) -> Vec<FnId> {
    let mut entries: Vec<FnId> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.name == "main"
                && f.owner == FnOwner::Free
                && project.module(f.module).parent.is_none()
        })
        .map(|(i, _)| FnId(i as u32))
        .collect();
    entries.sort_by_key(|&f| project.function(f).qual_path.starts_with("build_script"));
    entries
}

pub fn program(project: &Project, facts: &Facts, graph: &CallGraph, entry: FnId) -> Program {
    let mut w = walker::Walker::new(project, facts, graph, entry);
    w.walk(entry);
    let mut program = Program::default();
    connect::connect(&w, &mut program);
    program.unreached = w.unreached(entry);
    // Spawn links come first, then the channels.
    let mut links = std::mem::take(&mut w.links);
    links.append(&mut program.links);
    program.links = links;
    program.threads = w.threads;
    program
}
