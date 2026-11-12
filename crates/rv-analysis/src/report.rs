//! A compact Markdown summary of a project for readers who do not look at the
//! diagrams — mainly LLM assistants starting to work on unfamiliar code. Every
//! statement carries a `file:line` and the names to search for.

use rv_ir::{FnId, Project};

use crate::{CallGraph, Facts, Program, entry_points, program};

mod comm;
mod project;
mod threads;
mod writer;

/// Sections of the report; header, names to search for and limits are
/// always included.
#[derive(Clone, Copy, Debug)]
pub struct Parts {
    pub overview: bool,
    pub threads: bool,
    pub modules: bool,
    pub hot_spots: bool,
}

impl Parts {
    pub const ALL: Parts = Parts {
        overview: true,
        threads: true,
        modules: true,
        hot_spots: true,
    };
}

/// The report for the whole project.
pub fn report(project: &Project) -> String {
    let facts = Facts::new(project);
    let calls = CallGraph::new(project);
    let programs = programs(project, &facts, &calls);
    report_with(project, &facts, &calls, &programs, Parts::ALL)
}

/// The thread model of every binary (build scripts left out).
pub fn programs(project: &Project, facts: &Facts, calls: &CallGraph) -> Vec<(FnId, Program)> {
    entry_points(project)
        .into_iter()
        .filter(|&e| !project.function(e).qual_path.starts_with("build_script"))
        .map(|e| (e, program(project, facts, calls, e)))
        .collect()
}

/// Selected sections, from analyses that are already done (for callers that
/// keep them, like the MCP server).
pub fn report_with(
    project: &Project,
    facts: &Facts,
    calls: &CallGraph,
    programs: &[(FnId, Program)],
    parts: Parts,
) -> String {
    let mut r = writer::Report::new(project);
    r.header();
    if parts.overview {
        r.overview();
    }
    if parts.threads {
        for (entry, program) in programs {
            r.threads(*entry, program);
        }
    }
    if parts.modules {
        r.modules(facts, calls);
    }
    if parts.hot_spots {
        r.hot_spots(calls);
    }
    r.keywords();
    r.limits();
    r.out
}
