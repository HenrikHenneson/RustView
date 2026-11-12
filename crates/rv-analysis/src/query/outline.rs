//! The nesting of one function as indented text.

use std::collections::BTreeMap;
use std::fmt::Write;

use rv_ir::{Body, FnId, FrameRole, Node, NodeId, NodeKind, Project, StructKind};

use crate::Facts;
use crate::locate::Loc;

/// Lines of an outline before it is cut.
const MAX_OUTLINE: usize = 400;

/// The nesting of `f`: loops, branches, closures, thread lanes and exits,
/// one line each with its line range. With `statements`, also the plain
/// statements.
pub fn outline(project: &Project, facts: &Facts, f: FnId, statements: bool) -> String {
    let loc = Loc::new(project);
    let func = project.function(f);
    let Some(body) = &func.body else {
        return format!("{} has no body.", loc.func(f));
    };
    let mut o = Outline {
        body,
        marks: marks(body, facts, f),
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

/// What is noted behind the line of a node: exits and thread starts.
fn marks(body: &Body, facts: &Facts, f: FnId) -> BTreeMap<NodeId, Vec<String>> {
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
    marks
}

struct Outline<'b> {
    body: &'b Body,
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
            NodeKind::Structure { kind, .. } => self.structure(id, &n, kind, depth),
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

    /// A structure with its frames; the cases of a branch are named when
    /// there are several.
    fn structure(&mut self, id: NodeId, n: &Node, kind: &StructKind, depth: usize) {
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
            .filter(|&&c| case_label(self.body.node(c)).is_some())
            .count();
        for &c in &n.children {
            match case_label(self.body.node(c)) {
                Some(label) if cases > 1 => {
                    self.push(depth, c, &label);
                    self.node(c, depth + 1);
                }
                _ => self.node(c, depth),
            }
        }
    }
}

/// `pattern =>`, `pattern if guard =>` or `else =>` for the frame of a case.
fn case_label(frame: &Node) -> Option<String> {
    match &frame.kind {
        NodeKind::Frame {
            role: FrameRole::Case { pattern, guard },
        } => Some(match guard {
            Some(g) => format!("{pattern} if {g} =>"),
            None => format!("{pattern} =>"),
        }),
        NodeKind::Frame {
            role: FrameRole::Else,
        } => Some("else =>".into()),
        _ => None,
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
