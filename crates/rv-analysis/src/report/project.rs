//! The project as a whole: packages and size, modules and how they call
//! each other, the largest and the most called functions.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use rv_ir::{FnId, Severity};

use super::writer::{MAX_LIST, Report, plural};
use crate::locate::short;
use crate::{CallGraph, Facts, ModuleGraph, crates, entry_points, module_graph};

/// Modules listed per crate.
const MAX_MODULES: usize = 16;
/// Called modules named per module.
const MAX_CALLED: usize = 6;
/// Loading problems listed.
const MAX_PROBLEMS: usize = 5;

impl Report<'_> {
    pub(super) fn overview(&mut self) {
        self.line("## Overview");
        self.line("");
        self.packages();
        self.size();
        self.tests();
        self.binaries();
        self.loading_problems();
        self.line("");
    }

    /// One line per package with its targets.
    fn packages(&mut self) {
        let p = self.project;
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
                self.loc.path(&pkg.manifest_path)
            ));
        }
    }

    /// Files, lines and functions of the project.
    fn size(&mut self) {
        let p = self.project;
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
    }

    /// Test code of all packages; it is only counted.
    fn tests(&mut self) {
        let mut tests = rv_ir::TestSummary::default();
        let p = self.project;
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
    }

    /// The `main` functions of the binaries (build scripts left out).
    fn binaries(&mut self) {
        let p = self.project;
        let entries: Vec<FnId> = entry_points(p)
            .into_iter()
            .filter(|&f| !p.function(f).qual_path.starts_with("build_script"))
            .collect();
        if entries.is_empty() {
            self.line("- entry points: none (library only)");
        } else {
            let list: Vec<String> = entries.iter().map(|&f| self.loc.func(f)).collect();
            self.line(format!("- entry points: {}", list.join(", ")));
        }
    }

    /// Warnings and errors from loading the project.
    fn loading_problems(&mut self) {
        let p = self.project;
        let problems: Vec<_> = p
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info)
            .collect();
        if problems.is_empty() {
            return;
        }
        self.line(format!("- loading problems: {}", problems.len()));
        for d in problems.iter().take(MAX_PROBLEMS) {
            let at = match (&d.span, &d.file) {
                (Some(s), _) => format!("{}: ", self.loc.at(s)),
                (None, Some(f)) => format!("{}: ", self.loc.path(f)),
                _ => String::new(),
            };
            self.line(format!("  - {at}{}", d.message));
        }
    }

    pub(super) fn modules(&mut self, facts: &Facts, calls: &CallGraph) {
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
                let text = self.module(&g, i);
                self.line(text);
            }
            self.more(g.nodes.len(), MAX_MODULES);
            self.line("");
        }
    }

    /// One line about module `i` of `g`: its size and the modules it calls.
    fn module(&self, g: &ModuleGraph, i: usize) -> String {
        let p = self.project;
        let n = &g.nodes[i];
        let file = n
            .modules
            .first()
            .map(|&m| self.loc.path(&p.file(p.module(m).file).path))
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
                .take(MAX_CALLED)
                .map(|&(to, n)| format!("`{}` ×{n}", g.nodes[to].key))
                .collect();
            let _ = write!(text, " → {}", list.join(", "));
            if out.len() > MAX_CALLED {
                let _ = write!(text, " +{}", out.len() - MAX_CALLED);
            }
        }
        text
    }

    pub(super) fn hot_spots(&mut self, calls: &CallGraph) {
        let p = self.project;
        let mut sized: Vec<(FnId, u32, u32)> = p
            .functions
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let body = f.body.as_ref()?;
                Some((
                    FnId(i as u32),
                    f.span.line_count(),
                    crate::maintainability::nesting(body),
                ))
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
            self.line(format!("- {}: {lines} lines, depth {d}", self.loc.func(f)));
        }
        self.line("");
        self.most_called(calls);
    }

    /// The functions with the most distinct callers, if any has several.
    fn most_called(&mut self, calls: &CallGraph) {
        let p = self.project;
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
                self.line(format!("- {}: {n} callers", self.loc.func(f)));
            }
            self.line("");
        }
    }
}
