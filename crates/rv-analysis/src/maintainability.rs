//! Maintainability: how much of a project stays within limits that keep code
//! readable and changeable. Three things are measured, all from the
//! structure RustView already has:
//!
//! - **functions**: length, branching complexity, nesting depth and number
//!   of parameters;
//! - **files**: length;
//! - **modules**: groups of modules that call each other in a cycle.
//!
//! Every measure has a *target* (fine below it) and a *limit* (twice the
//! target or the usual lint default; above it the code should be split).
//! The score weighs code by its lines, so one huge function counts as much
//! as the many small ones it could be split into.

use rv_ir::{
    Body, ExitKind, FileId, FnId, FrameRole, Function, NodeId, NodeKind, Project, StmtKind,
    StructKind,
};

use crate::CallGraph;

mod cycles;
mod markdown;

pub use cycles::{Cycle, CycleLink};
pub use markdown::maintainability_report;

/// Where a value stands against its [`Limit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    OverTarget,
    OverLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    /// Values up to this are fine.
    pub target: u32,
    /// Values above this should be fixed.
    pub max: u32,
}

impl Limit {
    pub fn level(self, value: u32) -> Level {
        if value > self.max {
            Level::OverLimit
        } else if value > self.target {
            Level::OverTarget
        } else {
            Level::Ok
        }
    }
}

/// What is measured on a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    Lines,
    Complexity,
    Nesting,
    Params,
}

impl Measure {
    pub const ALL: [Measure; 4] = [
        Measure::Lines,
        Measure::Complexity,
        Measure::Nesting,
        Measure::Params,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Measure::Lines => "lines",
            Measure::Complexity => "complexity",
            Measure::Nesting => "nesting",
            Measure::Params => "parameters",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Lines of a function body.
    pub fn_lines: Limit,
    /// See [`complexity`].
    pub complexity: Limit,
    /// See [`nesting`].
    pub nesting: Limit,
    /// Parameters without `self`.
    pub params: Limit,
    /// Lines of a source file.
    pub file_lines: Limit,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            fn_lines: Limit {
                target: 60,
                max: 120,
            },
            complexity: Limit {
                target: 15,
                max: 30,
            },
            nesting: Limit { target: 4, max: 6 },
            params: Limit { target: 5, max: 7 },
            file_lines: Limit {
                target: 500,
                max: 1000,
            },
        }
    }
}

impl Limits {
    pub fn of(&self, measure: Measure) -> Limit {
        match measure {
            Measure::Lines => self.fn_lines,
            Measure::Complexity => self.complexity,
            Measure::Nesting => self.nesting,
            Measure::Params => self.params,
        }
    }
}

/// The measures of one function with a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnMeasures {
    pub f: FnId,
    pub lines: u32,
    pub complexity: u32,
    pub nesting: u32,
    pub params: u32,
}

impl FnMeasures {
    fn new(f: FnId, func: &Function, body: &Body) -> Self {
        FnMeasures {
            f,
            lines: func.body_span.as_ref().unwrap_or(&func.span).line_count(),
            complexity: complexity(body),
            nesting: nesting(body),
            params: func.sig.params.iter().filter(|p| p.pat != "self").count() as u32,
        }
    }

    pub fn value(&self, measure: Measure) -> u32 {
        match measure {
            Measure::Lines => self.lines,
            Measure::Complexity => self.complexity,
            Measure::Nesting => self.nesting,
            Measure::Params => self.params,
        }
    }

    /// The worst level of its measures.
    pub fn level(&self, limits: &Limits) -> Level {
        Measure::ALL
            .iter()
            .map(|&m| limits.of(m).level(self.value(m)))
            .max()
            .unwrap_or(Level::Ok)
    }

    /// How far the worst measure is over its limit (1.0 = at the limit).
    pub fn excess(&self, limits: &Limits) -> f32 {
        Measure::ALL
            .iter()
            .map(|&m| self.value(m) as f32 / limits.of(m).max.max(1) as f32)
            .fold(0.0, f32::max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeasures {
    pub file: FileId,
    pub lines: u32,
}

/// Lines of code by [`Level`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shares {
    pub ok: u64,
    pub over_target: u64,
    pub over_limit: u64,
}

impl Shares {
    fn add(&mut self, level: Level, lines: u32) {
        let slot = match level {
            Level::Ok => &mut self.ok,
            Level::OverTarget => &mut self.over_target,
            Level::OverLimit => &mut self.over_limit,
        };
        *slot += u64::from(lines);
    }

    pub fn total(&self) -> u64 {
        self.ok + self.over_target + self.over_limit
    }

    /// Percent of `part` in the total (100 for an empty total).
    pub fn percent(&self, part: u64) -> u32 {
        match self.total() {
            0 => 100,
            total => (part * 100 / total) as u32,
        }
    }

    /// 0–100: code within the targets counts fully, code between target and
    /// limit half, code over a limit not at all.
    pub fn score(&self) -> u32 {
        match self.total() {
            0 => 100,
            total => ((self.ok * 2 + self.over_target) * 50 / total) as u32,
        }
    }
}

/// 0–100 each; `total` weighs functions 60 %, files 20 % and modules 20 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Score {
    pub functions: u32,
    pub files: u32,
    pub modules: u32,
    pub total: u32,
}

impl Score {
    /// School grade from the total: A (≥ 90) to E (< 50).
    pub fn grade(&self) -> char {
        match self.total {
            90.. => 'A',
            80.. => 'B',
            65.. => 'C',
            50.. => 'D',
            _ => 'E',
        }
    }
}

/// The maintainability of a project.
#[derive(Clone, Debug)]
pub struct Maintainability {
    pub limits: Limits,
    /// Functions with a body, worst first.
    pub functions: Vec<FnMeasures>,
    /// Files with analyzed modules, longest first.
    pub files: Vec<FileMeasures>,
    /// Modules that call each other in a cycle, largest group first.
    pub cycles: Vec<Cycle>,
    /// Modules that contain functions.
    pub modules: usize,
}

impl Maintainability {
    pub fn new(project: &Project, calls: &CallGraph) -> Self {
        Self::with_limits(project, calls, Limits::default())
    }

    pub fn with_limits(project: &Project, calls: &CallGraph, limits: Limits) -> Self {
        let mut functions: Vec<FnMeasures> = project
            .functions
            .iter()
            .enumerate()
            .filter_map(|(i, func)| {
                let body = func.body.as_ref()?;
                Some(FnMeasures::new(FnId(i as u32), func, body))
            })
            .collect();
        functions.sort_by(|a, b| {
            b.excess(&limits)
                .total_cmp(&a.excess(&limits))
                .then(a.f.cmp(&b.f))
        });
        Maintainability {
            limits,
            functions,
            files: files(project),
            cycles: cycles::find(project, calls),
            modules: project
                .modules
                .iter()
                .filter(|m| !m.functions.is_empty())
                .count(),
        }
    }

    pub fn fn_level(&self, f: &FnMeasures) -> Level {
        f.level(&self.limits)
    }

    pub fn file_level(&self, f: &FileMeasures) -> Level {
        self.limits.file_lines.level(f.lines)
    }

    /// Function lines by level.
    pub fn fn_shares(&self) -> Shares {
        let mut shares = Shares::default();
        for f in &self.functions {
            shares.add(self.fn_level(f), f.lines);
        }
        shares
    }

    /// File lines by level.
    pub fn file_shares(&self) -> Shares {
        let mut shares = Shares::default();
        for f in &self.files {
            shares.add(self.file_level(f), f.lines);
        }
        shares
    }

    /// Modules that are part of a cycle.
    pub fn modules_in_cycles(&self) -> usize {
        self.cycles.iter().map(|c| c.modules.len()).sum()
    }

    pub fn score(&self) -> Score {
        let functions = self.fn_shares().score();
        let files = self.file_shares().score();
        let modules = match self.modules {
            0 => 100,
            n => (100 * n.saturating_sub(self.modules_in_cycles()) / n) as u32,
        };
        Score {
            functions,
            files,
            modules,
            total: (functions * 60 + files * 20 + modules * 20) / 100,
        }
    }

    /// Functions and files over a limit plus module cycles: what a strict
    /// check (`--gate`) does not accept.
    pub fn violations(&self) -> usize {
        let fns = self
            .functions
            .iter()
            .filter(|f| self.fn_level(f) == Level::OverLimit);
        let files = self
            .files
            .iter()
            .filter(|f| self.file_level(f) == Level::OverLimit);
        fns.count() + files.count() + self.cycles.len()
    }
}

/// Files that hold analyzed modules, longest first.
fn files(project: &Project) -> Vec<FileMeasures> {
    let mut ids: Vec<FileId> = project.modules.iter().map(|m| m.file).collect();
    ids.sort();
    ids.dedup();
    let mut files: Vec<FileMeasures> = ids
        .into_iter()
        .map(|file| FileMeasures {
            file,
            lines: project.file(file).text.lines().count() as u32,
        })
        .collect();
    files.sort_by_key(|f| (std::cmp::Reverse(f.lines), f.file));
    files
}

/// Nesting depth of structures in a body: loops, branches, closures and
/// blocks inside each other (the function itself is 0).
pub fn nesting(body: &Body) -> u32 {
    fn below(body: &Body, id: NodeId, depth: u32) -> u32 {
        let node = body.node(id);
        let own = match node.kind {
            NodeKind::Structure { .. } if id != body.root => depth + 1,
            _ => depth,
        };
        node.children
            .iter()
            .map(|&c| below(body, c, own))
            .max()
            .unwrap_or(0)
            .max(own)
    }
    below(body, body.root, 0)
}

/// Branching complexity in the spirit of "cognitive complexity": every loop,
/// `if`, `match` and `let … else` counts 1 plus its nesting level, every
/// further `else if` / `else` and every labeled `break` / `continue` counts 1.
/// Closures and `async` blocks only deepen the nesting. A flat sequence of
/// statements is 0, however long.
pub fn complexity(body: &Body) -> u32 {
    fn below(body: &Body, id: NodeId, level: u32) -> u32 {
        let node = body.node(id);
        let (own, inner) = match &node.kind {
            NodeKind::Structure { kind, .. } => structure_cost(body, id, kind, level),
            NodeKind::Stmt {
                kind:
                    StmtKind::Exit(
                        ExitKind::Break { label: Some(_) } | ExitKind::Continue { label: Some(_) },
                    ),
            } => (1, level),
            NodeKind::Stmt { .. } | NodeKind::Frame { .. } => (0, level),
        };
        own + node
            .children
            .iter()
            .map(|&c| below(body, c, inner))
            .sum::<u32>()
    }
    below(body, body.root, 0)
}

/// (cost of the structure itself, nesting level of what is inside it)
fn structure_cost(body: &Body, id: NodeId, kind: &StructKind, level: u32) -> (u32, u32) {
    match kind {
        StructKind::Function | StructKind::Block { .. } | StructKind::Unsafe => (0, level),
        StructKind::Closure { .. } | StructKind::Async { .. } => (0, level + 1),
        StructKind::LetElse => (1, level + 1),
        StructKind::Loop { .. }
        | StructKind::While { .. }
        | StructKind::For { .. }
        | StructKind::Match => (1 + level, level + 1),
        StructKind::If => {
            let branches = body
                .node(id)
                .children
                .iter()
                .filter(|&&c| {
                    matches!(
                        body.node(c).kind,
                        NodeKind::Frame {
                            role: FrameRole::Case { .. } | FrameRole::Else
                        }
                    )
                })
                .count() as u32;
            (level + branches.max(1), level + 1)
        }
    }
}
