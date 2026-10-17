//! Data flow over local variables (syntactic def-use approximation).
//!
//! Names are resolved through the nested scopes of the body (frames open a
//! scope, `let` shadows). Each binding has a current *writer*: the node that
//! defined it or last changed it. A read gets a [`EdgeKind::Data`] edge from
//! the writer; a change (`=`, `+=`, `&mut`) gets a [`EdgeKind::Mutation`]
//! edge and becomes the new writer. Method calls on `mut` bindings may or
//! may not mutate ([`EdgeKind::PossibleMutation`]).
//!
//! Seen from outside, a structure (or a statement with embedded closures)
//! that changes a variable is the writer afterwards — like an output tunnel.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use rv_ir::{Body, FrameRole, NodeId, NodeKind, UseMode, VarDef};

#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// Node whose `defs` introduced the binding.
    pub def: NodeId,
    pub mutable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// The value is read.
    Data,
    /// The value is changed (`=`, `+=`, `&mut`): orders the two nodes.
    Mutation,
    /// Method call on a `mut` binding: changes it if the method takes `&mut self`.
    PossibleMutation,
    /// The value written in one loop iteration is read in the next one
    /// (LabVIEW: shift register). From the last writer in the loop to the
    /// first use in the loop.
    LoopCarried,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// Index into [`DataFlow::bindings`].
    pub binding: usize,
    pub kind: EdgeKind,
    /// `false` if the use was guessed from an unexpanded macro.
    pub certain: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DataFlow {
    pub bindings: Vec<Binding>,
    pub edges: Vec<Edge>,
    /// (loop, binding): values carried from one iteration to the next.
    pub shift_registers: Vec<(NodeId, usize)>,
}

impl DataFlow {
    pub fn binding(&self, edge: &Edge) -> &Binding {
        &self.bindings[edge.binding]
    }

    /// One line per edge, sorted; for tests and debugging.
    pub fn dump(&self, body: &Body) -> String {
        let name = |id: NodeId| {
            let n = body.node(id);
            match &n.kind {
                NodeKind::Frame {
                    role: FrameRole::Body,
                } => "<body>".to_string(),
                _ if n.label.is_empty() => format!("<{}>", id.0),
                _ => n.label.clone(),
            }
        };
        let mut lines: Vec<String> = self
            .edges
            .iter()
            .map(|e| {
                let kind = match e.kind {
                    EdgeKind::Data => "",
                    EdgeKind::Mutation => " [mut]",
                    EdgeKind::PossibleMutation => " [mut?]",
                    EdgeKind::LoopCarried => " [loop]",
                };
                let guess = if e.certain { "" } else { " (guess)" };
                format!(
                    "{}: {} -> {}{kind}{guess}",
                    self.binding(e).name,
                    name(e.from),
                    name(e.to)
                )
            })
            .collect();
        lines.sort();
        let mut out = String::new();
        for l in lines {
            let _ = writeln!(out, "{l}");
        }
        out
    }
}

pub fn data_flow(body: &Body) -> DataFlow {
    let mut a = Analyzer {
        body,
        flow: DataFlow::default(),
        scopes: Vec::new(),
        writer: Vec::new(),
        decl_depth: Vec::new(),
        markers: Vec::new(),
        seen: HashSet::new(),
    };
    a.walk(body.root);
    a.flow
}

struct Analyzer<'a> {
    body: &'a Body,
    flow: DataFlow,
    /// Name -> binding, innermost scope last.
    scopes: Vec<HashMap<String, usize>>,
    /// Current writer per binding.
    writer: Vec<NodeId>,
    /// Number of open markers when the binding was declared.
    decl_depth: Vec<usize>,
    /// Structures (and statements with embedded structures) being walked.
    markers: Vec<Marker>,
    seen: HashSet<Edge>,
}

struct Marker {
    node: NodeId,
    /// Bindings changed while the marker was open.
    written: HashSet<usize>,
    /// First node inside that reads a binding declared outside.
    first_read: HashMap<usize, NodeId>,
}

impl Analyzer<'_> {
    fn walk(&mut self, id: NodeId) {
        let node = self.body.node(id);
        match &node.kind {
            NodeKind::Structure { .. } => {
                // Header expression: scrutinee, iterator, loop condition.
                self.uses(id);
                let is_root = id == self.body.root;
                if is_root {
                    self.scopes.push(HashMap::new());
                    self.define(&node.defs, id);
                }
                self.open(id);
                for &f in &node.children {
                    self.walk(f);
                }
                self.close();
                // `let x = match ...`: x exists after the structure.
                if !is_root {
                    self.define(&node.defs, id);
                }
            }
            NodeKind::Frame { .. } => {
                self.scopes.push(HashMap::new());
                // Case condition / guard, then the bindings of its pattern.
                self.uses(id);
                self.define(&node.defs, id);
                for &c in &node.children {
                    self.walk(c);
                }
                self.scopes.pop();
            }
            NodeKind::Stmt { .. } => {
                // Embedded structures (closures, async blocks) first: they are
                // created while the statement is evaluated.
                self.open(id);
                for &c in &node.children {
                    self.walk(c);
                }
                self.close();
                self.uses(id);
                self.define(&node.defs, id);
            }
        }
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn define(&mut self, defs: &[VarDef], at: NodeId) {
        for d in defs {
            let id = self.flow.bindings.len();
            self.flow.bindings.push(Binding {
                name: d.name.clone(),
                def: at,
                mutable: d.mutable,
            });
            self.writer.push(at);
            self.decl_depth.push(self.markers.len());
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(d.name.clone(), id);
            }
        }
    }

    fn uses(&mut self, id: NodeId) {
        for u in &self.body.node(id).uses {
            let Some(b) = self.resolve(&u.name) else {
                continue; // not a local variable (function, static, ...)
            };
            let mutable = self.flow.bindings[b].mutable;
            let (kind, writes) = match u.mode {
                UseMode::Read => (EdgeKind::Data, false),
                UseMode::MethodRecv if mutable => (EdgeKind::PossibleMutation, true),
                UseMode::MethodRecv => (EdgeKind::Data, false),
                UseMode::Assign | UseMode::CompoundAssign | UseMode::RefMut => {
                    (EdgeKind::Mutation, true)
                }
            };
            self.edge(self.writer[b], id, b, kind, u.certain);
            if u.mode != UseMode::Assign {
                // Remember where each open structure first reads it.
                let decl = self.decl_depth[b];
                for m in self.markers.iter_mut().skip(decl) {
                    m.first_read.entry(b).or_insert(id);
                }
            }
            if writes {
                self.writer[b] = id;
                if let Some(m) = self.markers.last_mut() {
                    m.written.insert(b);
                }
            }
        }
    }

    fn edge(&mut self, from: NodeId, to: NodeId, binding: usize, kind: EdgeKind, certain: bool) {
        if from == to {
            return;
        }
        let edge = Edge {
            from,
            to,
            binding,
            kind,
            certain,
        };
        if self.seen.insert(edge.clone()) {
            self.flow.edges.push(edge);
        }
    }

    fn open(&mut self, node: NodeId) {
        self.markers.push(Marker {
            node,
            written: HashSet::new(),
            first_read: HashMap::new(),
        });
    }

    /// Variables from outside that were changed inside are, from now on,
    /// written by the closed structure.
    fn close(&mut self) {
        let Some(m) = self.markers.pop() else { return };
        let depth = self.markers.len();
        let is_loop = matches!(&self.body.node(m.node).kind, NodeKind::Structure { kind, .. } if kind.is_loop());
        if is_loop {
            let mut carried: Vec<usize> = m
                .written
                .iter()
                .copied()
                .filter(|b| self.decl_depth[*b] <= depth && m.first_read.contains_key(b))
                .collect();
            carried.sort();
            for b in carried {
                // Last writer inside the loop feeds the first read of the next round.
                self.edge(
                    self.writer[b],
                    m.first_read[&b],
                    b,
                    EdgeKind::LoopCarried,
                    true,
                );
                self.flow.shift_registers.push((m.node, b));
            }
        }
        for b in m.written {
            if self.decl_depth[b] <= depth {
                self.writer[b] = m.node;
                if let Some(parent) = self.markers.last_mut() {
                    parent.written.insert(b);
                }
            }
        }
    }
}
