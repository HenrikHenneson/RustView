//! Follows a value (channel end, shared handle, join handle) from its
//! binding to the places where it is used: through clones, call arguments
//! into parameters, struct fields into the type's methods and return values
//! into callers.

use std::collections::HashSet;

use rv_ir::{Body, CallKind, CallSite, FnId, Node, NodeId, Project};

use super::callgraph::CallEdge;
use super::facts::Facts;
use super::model::Site;
use super::resolver::self_type;
use super::walker::Walker;
use crate::DataFlow;

/// How many function boundaries a value is followed across.
const MAX_DEPTH: usize = 10;

/// Where a value ended up.
pub(super) struct Trail {
    /// Places where it is used, in discovery order.
    pub sites: Vec<Site>,
    /// Declared types of the parameters it was passed into.
    pub types: Vec<String>,
}

impl Walker<'_> {
    /// Places where the bindings `start` of `f` are used.
    pub(super) fn trail(&self, f: FnId, start: &[usize]) -> Trail {
        let mut follower = Follower {
            w: self,
            seen_bindings: HashSet::new(),
            seen_fields: HashSet::new(),
            trail: Trail {
                sites: Vec::new(),
                types: Vec::new(),
            },
        };
        follower.follow(f, start, 0);
        follower.trail
    }
}

/// Follows one value through the program.
struct Follower<'a> {
    w: &'a Walker<'a>,
    /// Bindings already followed, per function.
    seen_bindings: HashSet<(FnId, usize)>,
    /// (type, field) already followed.
    seen_fields: HashSet<(String, String)>,
    trail: Trail,
}

/// The function a value is currently followed in.
struct Scope<'a> {
    f: FnId,
    body: &'a Body,
    flow: &'a DataFlow,
    /// Bindings of `f` that hold the value; clones are added on the way.
    bindings: HashSet<usize>,
    /// Function boundaries crossed so far.
    depth: usize,
}

impl Scope<'_> {
    /// `name` used in `node` is one of the followed bindings.
    fn ours(&self, node: NodeId, name: &str) -> bool {
        self.flow
            .binding_at(node, name)
            .is_some_and(|b| self.bindings.contains(&b))
    }

    /// The new binding if `id` is `let b = a.clone()` / `let b = Arc::clone(&a)`
    /// of a followed binding.
    fn clone_of_ours(&self, id: NodeId) -> Option<usize> {
        let node = self.body.node(id);
        let is_clone = node.calls.iter().any(|c| {
            c.name() == "clone"
                && (c.receiver.as_deref().is_some_and(|r| self.ours(id, r))
                    || c.args
                        .first()
                        .cloned()
                        .flatten()
                        .is_some_and(|a| self.ours(id, &a)))
        });
        if is_clone && node.defs.len() == 1 {
            self.flow.defined_at(id, &node.defs[0].name)
        } else {
            None
        }
    }
}

impl<'a> Follower<'a> {
    /// Records the places where the bindings `start` of `f` are used.
    fn follow(&mut self, f: FnId, start: &[usize], depth: usize) {
        let (Some(body), Some(flow)) = (self.w.body(f), self.w.facts.flow(f)) else {
            return;
        };
        let bindings: HashSet<usize> = start
            .iter()
            .copied()
            .filter(|b| self.seen_bindings.insert((f, *b)))
            .collect();
        if bindings.is_empty() || depth > MAX_DEPTH {
            return;
        }
        for &b in &bindings {
            if flow.bindings[b].def == body.root
                && let Some(ty) = body
                    .node(body.root)
                    .defs
                    .iter()
                    .find(|d| d.name == flow.bindings[b].name)
                    .and_then(|d| d.ty.clone())
            {
                self.trail.types.push(ty);
            }
        }
        let mut scope = Scope {
            f,
            body,
            flow,
            bindings,
            depth,
        };
        for n in 0..body.nodes.len() {
            self.node(&mut scope, NodeId(n as u32));
        }
    }

    /// What node `id` does with the followed bindings, if it uses one.
    fn node(&mut self, scope: &mut Scope<'a>, id: NodeId) {
        let node = scope.body.node(id);
        if !node.uses.iter().any(|u| scope.ours(id, &u.name)) {
            return;
        }
        // `let b = a.clone()` / `Arc::clone(&a)` keeps following.
        if let Some(new) = scope.clone_of_ours(id) {
            scope.bindings.insert(new);
            return;
        }
        let mut handled = self.method_calls(scope, id, node);
        handled |= self.passed_to_calls(scope, id, node);
        handled |= self.stored_in_fields(scope, id, node);
        handled |= self.returned_to_callers(scope, id, node);
        if !handled {
            self.sites(scope.f, id, None);
        }
    }

    /// Methods called on the value (or on one of its fields: `g.stand.lock()`)
    /// are operations. Returns whether there was one.
    fn method_calls(&mut self, scope: &Scope<'a>, id: NodeId, node: &Node) -> bool {
        let mut handled = false;
        for call in &node.calls {
            if !call.receiver.as_deref().is_some_and(|r| scope.ours(id, r)) {
                continue;
            }
            let method = call.path.first().cloned();
            if call.field.is_none() && method.as_deref() == Some("clone") {
                continue;
            }
            let op = match (&call.field, method) {
                (Some(f), Some(m)) => Some(format!("{f}.{m}")),
                (_, m) => m,
            };
            self.sites(scope.f, id, op);
            handled = true;
        }
        handled
    }

    /// Handed to a project function: continues with the parameter. Returns
    /// whether it was followed.
    fn passed_to_calls(&mut self, scope: &Scope<'a>, id: NodeId, node: &Node) -> bool {
        let w = self.w;
        let mut handled = false;
        for (ci, call) in node.calls.iter().enumerate() {
            let Some(callee) = w.graph.callee(scope.f, id, ci) else {
                continue;
            };
            for (k, arg) in call.args.iter().enumerate() {
                if arg.as_deref().is_some_and(|a| scope.ours(id, a))
                    && let Some(param) = param_binding(w.project, w.facts, callee, call.kind, k)
                {
                    self.follow(callee, &[param], scope.depth + 1);
                    handled = true;
                }
            }
        }
        handled
    }

    /// Stored in a struct field: used by the type's methods. Returns whether
    /// it was followed.
    fn stored_in_fields(&mut self, scope: &Scope<'a>, id: NodeId, node: &Node) -> bool {
        let mut handled = false;
        for u in &node.uses {
            let Some((strukt, field)) = &u.stored_in else {
                continue;
            };
            if !scope.ours(id, &u.name) {
                continue;
            }
            let ty = if strukt == "Self" {
                self_type(&self.w.project.function(scope.f).owner).map(str::to_string)
            } else {
                Some(strukt.clone())
            };
            if let Some(ty) = ty {
                // The storing thread holds it, the type's methods use it.
                self.sites(scope.f, id, Some(format!("stored in {ty}.{field}")));
                self.follow_field(&ty, field, scope.depth + 1);
                handled = true;
            }
        }
        handled
    }

    /// Returned: continues where the callers bind the result. Returns
    /// whether it was followed.
    fn returned_to_callers(&mut self, scope: &Scope<'a>, id: NodeId, node: &Node) -> bool {
        if !is_returned(scope.body, id) {
            return false;
        }
        let w = self.w;
        // Position in a returned tuple.
        let k = node
            .uses
            .iter()
            .position(|u| scope.ours(id, &u.name))
            .unwrap_or(0);
        let mut handled = false;
        for e in w.graph.edges.iter().filter(|e| e.callee == scope.f) {
            if let Some(b) = self.result_binding(e, k) {
                self.follow(e.caller, &[b], scope.depth + 1);
                handled = true;
            }
        }
        handled
    }

    /// The binding the caller gives to the result of the call `e`: the only
    /// one, or the `k`-th of a destructured tuple.
    fn result_binding(&self, e: &CallEdge, k: usize) -> Option<usize> {
        let body = self.w.body(e.caller)?;
        let flow = self.w.facts.flow(e.caller)?;
        let defs = &body.node(e.node).defs;
        let def = if defs.len() == 1 {
            defs.first()
        } else {
            defs.get(k)
        };
        flow.defined_at(e.node, &def?.name)
    }

    /// Uses of `self.<field>` in the methods of `ty`.
    fn follow_field(&mut self, ty: &str, field: &str, depth: usize) {
        if !self.seen_fields.insert((ty.to_string(), field.to_string())) {
            return;
        }
        let w = self.w;
        for (i, m) in w.project.functions.iter().enumerate() {
            if self_type(&m.owner) != Some(ty) {
                continue;
            }
            let id = FnId(i as u32);
            let (Some(body), Some(flow)) = (m.body.as_ref(), w.facts.flow(id)) else {
                continue;
            };
            for (nid, node, call) in field_calls(body, field) {
                let op = call.path.first().cloned();
                if op.as_deref() != Some("clone") {
                    self.sites(id, nid, op);
                    continue;
                }
                // `let g = self.field.clone()` (e.g. before spawning).
                if node.defs.len() == 1
                    && let Some(b) = flow.defined_at(nid, &node.defs[0].name)
                {
                    self.follow(id, &[b], depth + 1);
                }
            }
        }
    }

    /// One site per thread executing `node` of `f`.
    fn sites(&mut self, f: FnId, node: NodeId, op: Option<String>) {
        self.w.sites(f, node, op, &mut self.trail.sites);
    }
}

/// The method calls on `self.<field>` in a body, with their nodes.
fn field_calls<'b>(
    body: &'b Body,
    field: &'b str,
) -> impl Iterator<Item = (NodeId, &'b Node, &'b CallSite)> {
    body.nodes.iter().enumerate().flat_map(move |(n, node)| {
        node.calls
            .iter()
            .filter(move |call| {
                call.receiver.as_deref() == Some("self") && call.field.as_deref() == Some(field)
            })
            .map(move |call| (NodeId(n as u32), node, call))
    })
}

/// The node hands its value to the caller (`return x`, or the function's tail).
fn is_returned(body: &Body, node: NodeId) -> bool {
    let n = body.node(node);
    if matches!(
        n.kind,
        rv_ir::NodeKind::Stmt {
            kind: rv_ir::StmtKind::Exit(rv_ir::ExitKind::Return)
        }
    ) {
        return true;
    }
    n.value
        && n.parent
            .is_some_and(|p| body.node(p).parent == Some(body.root))
}

/// Binding of the parameter that receives argument `k` of a call.
fn param_binding(
    project: &Project,
    facts: &Facts,
    callee: FnId,
    kind: CallKind,
    k: usize,
) -> Option<usize> {
    let func = project.function(callee);
    let has_self = func.sig.params.first().is_some_and(|p| p.pat == "self");
    // `x.method(a)`: `a` is the parameter after `self`.
    let index = if kind == CallKind::Method && has_self {
        k + 1
    } else {
        k
    };
    let param = func.sig.params.get(index)?;
    let name = param.pat.trim_start_matches("mut ").trim();
    let body = func.body.as_ref()?;
    let flow = facts.flow(callee)?;
    flow.bindings
        .iter()
        .position(|b| b.def == body.root && b.name == name)
}
