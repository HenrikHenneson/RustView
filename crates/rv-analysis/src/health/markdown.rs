//! The health of a project as Markdown, worst findings first, every one
//! with a `file:line`.

use std::fmt::Write;

use rv_ir::{ModuleId, Project};

use super::{Cycle, FileHealth, FnHealth, Health, Level, Limit, Measure, Shares};
use crate::locate::Loc;

/// Functions or files listed per level.
const MAX_LIST: usize = 25;
/// Links listed per cycle.
const MAX_LINKS: usize = 6;

/// Markdown report of `health`.
pub fn health_report(project: &Project, health: &Health) -> String {
    let mut w = Writer {
        loc: Loc::new(project),
        health,
        out: String::new(),
    };
    w.header();
    w.limits();
    w.functions();
    w.files();
    w.cycles();
    w.out
}

struct Writer<'a> {
    loc: Loc<'a>,
    health: &'a Health,
    out: String,
}

impl Writer<'_> {
    fn line(&mut self, text: impl AsRef<str>) {
        self.out.push_str(text.as_ref());
        self.out.push('\n');
    }

    fn header(&mut self) {
        let name = self
            .loc
            .base
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let score = self.health.score();
        self.line(format!("# RustView code health: {name}"));
        self.line("");
        self.line(format!(
            "**Score {} / 100 (grade {})** — functions {}, files {}, modules {}. \
             {} finding{} over a limit.",
            score.total,
            score.grade(),
            score.functions,
            score.files,
            score.modules,
            self.health.violations(),
            plural(self.health.violations()),
        ));
        self.line("");
        self.line(
            "The score is the share of code within the targets below: code between \
             target and limit counts half, code over a limit not at all. Functions \
             weigh 60 %, files 20 %, modules outside of call cycles 20 %. Measured \
             on the syntax only; test code is not included.",
        );
        self.line("");
    }

    fn limits(&mut self) {
        let l = self.health.limits;
        self.line("| measure | target | limit |");
        self.line("|---|---|---|");
        let rows = [
            ("function length (lines of the body)", l.fn_lines),
            (
                "function complexity (branches and loops, weighted by nesting)",
                l.complexity,
            ),
            ("nesting depth (structures inside each other)", l.nesting),
            ("parameters (without `self`)", l.params),
            ("file length (lines)", l.file_lines),
        ];
        for (name, Limit { target, max }) in rows {
            self.line(format!("| {name} | {target} | {max} |"));
        }
        self.line("| modules calling each other in a cycle | 0 | 0 |");
        self.line("");
    }

    fn shares(&mut self, what: &str, count: usize, shares: Shares) {
        self.line(format!(
            "{count} {what}, {} lines: {} % within the targets, {} % over a target, \
             {} % over a limit.",
            shares.total(),
            shares.percent(shares.ok),
            shares.percent(shares.over_target),
            shares.percent(shares.over_limit),
        ));
        self.line("");
    }

    fn functions(&mut self) {
        let h = self.health;
        self.line("## Functions");
        self.line("");
        self.shares("functions", h.functions.len(), h.fn_shares());
        for (level, title) in [
            (Level::OverLimit, "Over a limit"),
            (Level::OverTarget, "Over a target"),
        ] {
            let found: Vec<&FnHealth> = h
                .functions
                .iter()
                .filter(|f| h.fn_level(f) == level)
                .collect();
            if found.is_empty() {
                continue;
            }
            self.line(format!("### {title} ({})", found.len()));
            self.line("");
            for f in found.iter().take(MAX_LIST) {
                let text = self.function(f);
                self.line(text);
            }
            self.more(found.len());
            self.line("");
        }
    }

    /// One function with its measures; what is over a limit is bold, what
    /// is within the target is left out.
    fn function(&self, f: &FnHealth) -> String {
        let mut text = format!("- {}:", self.loc.func(f.f));
        let mut first = true;
        for m in Measure::ALL {
            let value = f.value(m);
            let level = self.health.limits.of(m).level(value);
            if level == Level::Ok && m != Measure::Lines {
                continue;
            }
            let sep = if std::mem::replace(&mut first, false) {
                " "
            } else {
                ", "
            };
            let shown = match m {
                Measure::Lines => format!("{value} lines"),
                Measure::Params => format!("{value} parameters"),
                _ => format!("{} {value}", m.label()),
            };
            let _ = match level {
                Level::OverLimit => write!(text, "{sep}**{shown}**"),
                _ => write!(text, "{sep}{shown}"),
            };
        }
        text
    }

    fn more(&mut self, total: usize) {
        if total > MAX_LIST {
            self.line(format!("- … {} more", total - MAX_LIST));
        }
    }

    fn files(&mut self) {
        let h = self.health;
        self.line("## Files");
        self.line("");
        self.shares("files", h.files.len(), h.file_shares());
        let long: Vec<&FileHealth> = h
            .files
            .iter()
            .filter(|f| h.file_level(f) != Level::Ok)
            .collect();
        for f in long.iter().take(MAX_LIST) {
            let path = self.loc.path(&self.loc.project.file(f.file).path);
            let lines = match h.file_level(f) {
                Level::OverLimit => format!("**{} lines**", f.lines),
                _ => format!("{} lines", f.lines),
            };
            self.line(format!("- {path}: {lines}"));
        }
        self.more(long.len());
        if !long.is_empty() {
            self.line("");
        }
    }

    fn module(&self, m: ModuleId) -> String {
        format!("`{}`", self.loc.project.module(m).qual_name())
    }

    fn cycles(&mut self) {
        let h = self.health;
        self.line("## Module cycles");
        self.line("");
        if h.cycles.is_empty() {
            self.line(format!(
                "None: no group of the {} modules calls each other in a cycle.",
                h.modules
            ));
            return;
        }
        self.line(
            "Modules that reach each other through calls can only be understood and \
             changed together. Links are listed with the fewest call sites first: \
             removing those (moving the called function, or passing the value in) \
             is usually the cheapest way to break the cycle.",
        );
        self.line("");
        for c in &h.cycles {
            self.cycle(c);
        }
    }

    fn cycle(&mut self, c: &Cycle) {
        let names: Vec<String> = c.modules.iter().map(|&m| self.module(m)).collect();
        self.line(format!("- {}", names.join(", ")));
        for link in c.links.iter().take(MAX_LINKS) {
            let (f, node) = link.calls[0];
            self.line(format!(
                "  - {} → {}: {} call{}, e.g. {} in `{}`",
                self.module(link.from),
                self.module(link.to),
                link.calls.len(),
                plural(link.calls.len()),
                self.loc.node(f, node),
                self.loc.project.function(f).name,
            ));
        }
        if c.links.len() > MAX_LINKS {
            self.line(format!("  - … {} more links", c.links.len() - MAX_LINKS));
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
