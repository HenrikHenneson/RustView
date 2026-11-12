//! What the call graph view shows: the module graph of a crate with its
//! layout, and the calls of one module for the drill-down.

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2, pos2, vec2};
use rv_analysis::{CallGraph, Facts, ModuleEdge, ModuleGraph, ModuleNode};
use rv_ir::{FnId, Project};
use rv_layout::layered::layered;

use crate::canvas::bezier;

/// Height of a text line in a module box.
pub(super) const LINE: f32 = 16.0;
/// Space between the border of a module box and its text.
pub(super) const PAD: f32 = 7.0;
/// Estimated width of a character, for the box width.
const CHAR_WIDTH: f32 = 7.6;
const MIN_BOX_WIDTH: f32 = 120.0;
/// Space between the columns and between the boxes of a column.
const GAP: Vec2 = vec2(90.0, 16.0);
/// How far a back edge swings out to the left.
const BACK_EDGE_BEND: f32 = 60.0;
/// Segments an edge curve is drawn with.
const CURVE_STEPS: usize = 20;

/// Whole-project analysis, computed once per project version.
pub(super) struct Data {
    pub facts: Facts,
    pub calls: CallGraph,
    pub crates: Vec<String>,
    /// The crate of the first `main`.
    pub default_crate: usize,
}

impl Data {
    pub fn new(project: &Project) -> Self {
        let crates = rv_analysis::crates(project);
        let default_crate = rv_analysis::entry_points(project)
            .first()
            .and_then(|&f| {
                let c = project
                    .function(f)
                    .qual_path
                    .split("::")
                    .next()?
                    .to_string();
                crates.iter().position(|x| *x == c)
            })
            .unwrap_or(0);
        Data {
            facts: Facts::new(project),
            calls: CallGraph::new(project),
            crates,
            default_crate,
        }
    }
}

/// What a text line in a module box says (decides font and color).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind {
    /// The module's name.
    Name,
    /// Number of functions and lines.
    Size,
    /// The module starts threads.
    Threads,
}

/// Module graph of a crate with its layered layout (callers left, callees right).
pub(crate) struct ModuleLayout {
    pub graph: ModuleGraph,
    pub size: Vec2,
    /// Box of every node.
    boxes: Vec<Rect>,
    /// Text lines of every node.
    labels: Vec<Vec<(String, LineKind)>>,
    /// Curve of every edge.
    curves: Vec<Vec<Pos2>>,
}

impl ModuleLayout {
    pub fn new(
        project: &Project,
        calls: &CallGraph,
        facts: &Facts,
        crate_name: &str,
        depth: usize,
    ) -> Self {
        let graph = rv_analysis::module_graph(project, calls, facts, crate_name, depth);
        let labels: Vec<Vec<(String, LineKind)>> = graph
            .nodes
            .iter()
            .map(|n| node_lines(n, crate_name))
            .collect();
        let sizes: Vec<Vec2> = labels.iter().map(|lines| node_size(lines)).collect();
        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.from, e.to)).collect();
        let l = layered(&sizes, &edges, GAP);
        let boxes: Vec<Rect> = l
            .positions
            .iter()
            .zip(&sizes)
            .map(|(&pos, &size)| Rect::from_min_size(pos, size))
            .collect();
        let curves = graph
            .edges
            .iter()
            .map(|e| bezier(&connect(boxes[e.from], boxes[e.to]), CURVE_STEPS))
            .collect();
        ModuleLayout {
            graph,
            size: l.size,
            boxes,
            labels,
            curves,
        }
    }

    pub fn box_of(&self, node: usize) -> Rect {
        self.boxes[node]
    }

    pub fn labels(&self, node: usize) -> &[(String, LineKind)] {
        &self.labels[node]
    }

    /// The curve of an edge, by its index in the graph.
    pub fn curve(&self, edge: usize) -> &[Pos2] {
        &self.curves[edge]
    }

    pub fn curves(&self) -> impl Iterator<Item = &[Pos2]> {
        self.curves.iter().map(Vec::as_slice)
    }

    /// The node whose box contains `p`.
    pub fn node_at(&self, p: Pos2) -> Option<usize> {
        self.boxes.iter().position(|b| b.contains(p))
    }
}

/// Line width of an edge: grows with the number of calls.
pub(crate) fn edge_width(e: &ModuleEdge) -> f32 {
    1.0 + (e.calls.len() as f32).log2()
}

fn node_lines(n: &ModuleNode, crate_name: &str) -> Vec<(String, LineKind)> {
    let name = n
        .key
        .strip_prefix(&format!("{crate_name}::"))
        .unwrap_or(if n.key == crate_name {
            "(crate root)"
        } else {
            &n.key
        });
    let mut lines = vec![
        (name.to_string(), LineKind::Name),
        (
            format!("{} fns · {} lines", n.functions.len(), n.lines),
            LineKind::Size,
        ),
    ];
    if n.spawns > 0 {
        lines.push((
            format!("‖ starts threads ({})", n.spawns),
            LineKind::Threads,
        ));
    }
    lines
}

fn node_size(lines: &[(String, LineKind)]) -> Vec2 {
    let chars = lines
        .iter()
        .map(|(t, _)| t.chars().count())
        .max()
        .unwrap_or(10);
    vec2(
        (chars as f32 * CHAR_WIDTH + 2.0 * PAD).max(MIN_BOX_WIDTH),
        lines.len() as f32 * LINE + 2.0 * PAD,
    )
}

/// Bézier between two boxes: right side to left side when the target is to
/// the right, otherwise an arc around.
fn connect(a: Rect, b: Rect) -> [Pos2; 4] {
    if b.min.x > a.max.x {
        let (p0, p3) = (pos2(a.max.x, a.center().y), pos2(b.min.x, b.center().y));
        let dx = (p3.x - p0.x) * 0.5;
        [p0, p0 + vec2(dx, 0.0), p3 - vec2(dx, 0.0), p3]
    } else {
        // Back edge (cycle) or same column: leave below, come back above.
        let (p0, p3) = (pos2(a.center().x, a.max.y), pos2(b.center().x, b.min.y));
        let bend = BACK_EDGE_BEND;
        [p0, p0 + vec2(-bend, bend), p3 + vec2(-bend, -bend), p3]
    }
}

/// Calls between the module in focus and one other module.
pub(super) struct ModuleCalls {
    /// Node of the other module.
    pub other: usize,
    /// Number of call sites.
    pub total: usize,
    /// (caller, callee) with the number of call sites: the same pair called
    /// at several places is one entry.
    pub pairs: Vec<((FnId, FnId), usize)>,
}

/// A function of the module in focus.
pub(super) struct FnRow {
    pub f: FnId,
    /// Its path inside the module.
    pub name: String,
    /// Calls from and to other modules.
    pub calls_in: usize,
    pub calls_out: usize,
}

/// Drill-down into one module: who calls it, its functions, whom it calls.
pub(super) struct Focus {
    /// Node of the module.
    pub node: usize,
    pub incoming: Vec<ModuleCalls>,
    pub outgoing: Vec<ModuleCalls>,
    pub functions: Vec<FnRow>,
}

impl Focus {
    pub fn new(project: &Project, crate_name: &str, g: &ModuleGraph, node: usize) -> Self {
        let incoming = calls_by_module(g, node, true);
        let outgoing = calls_by_module(g, node, false);
        let n = &g.nodes[node];
        let module_prefix = format!(
            "{}::",
            n.key.strip_prefix(&format!("{crate_name}::")).unwrap_or("")
        );
        let functions = n
            .functions
            .iter()
            .map(|&f| {
                let label = fn_label(project, crate_name, f);
                FnRow {
                    f,
                    name: label
                        .strip_prefix(&module_prefix)
                        .unwrap_or(&label)
                        .to_string(),
                    calls_in: incoming.values().flatten().filter(|(_, c)| *c == f).count(),
                    calls_out: outgoing.values().flatten().filter(|(c, _)| *c == f).count(),
                }
            })
            .collect();
        Focus {
            node,
            incoming: incoming.into_iter().map(module_calls).collect(),
            outgoing: outgoing.into_iter().map(module_calls).collect(),
            functions,
        }
    }
}

/// Qualified path of a function without the crate name.
pub(super) fn fn_label(project: &Project, crate_name: &str, f: FnId) -> String {
    let q = &project.function(f).qual_path;
    q.strip_prefix(&format!("{crate_name}::"))
        .unwrap_or(q)
        .to_string()
}

/// Calls into (`incoming`) or out of a node, by the node on the other side.
fn calls_by_module(
    g: &ModuleGraph,
    node: usize,
    incoming: bool,
) -> BTreeMap<usize, Vec<(FnId, FnId)>> {
    let mut m: BTreeMap<usize, Vec<(FnId, FnId)>> = BTreeMap::new();
    for e in &g.edges {
        if incoming && e.to == node {
            m.entry(e.from).or_default().extend(e.calls.iter().copied());
        } else if !incoming && e.from == node {
            m.entry(e.to).or_default().extend(e.calls.iter().copied());
        }
    }
    m
}

fn module_calls((other, list): (usize, Vec<(FnId, FnId)>)) -> ModuleCalls {
    let mut pairs: Vec<((FnId, FnId), usize)> = Vec::new();
    for &pair in &list {
        match pairs.iter_mut().find(|(p, _)| *p == pair) {
            Some((_, n)) => *n += 1,
            None => pairs.push((pair, 1)),
        }
    }
    ModuleCalls {
        other,
        total: list.len(),
        pairs,
    }
}
