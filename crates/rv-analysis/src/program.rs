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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use rv_ir::{Body, CallKind, CallSite, FnId, FnOwner, NodeId, Project};

use crate::{Concurrency, DataFlow, SpawnKind};

/// Data flow and concurrency facts of every function.
pub struct Facts {
    flow: Vec<Option<DataFlow>>,
    conc: Vec<Option<Concurrency>>,
}

impl Facts {
    pub fn new(project: &Project) -> Self {
        let mut flow = Vec::with_capacity(project.functions.len());
        let mut conc = Vec::with_capacity(project.functions.len());
        for f in &project.functions {
            match &f.body {
                Some(body) => {
                    let fl = crate::data_flow(body);
                    conc.push(Some(crate::concurrency(body, &fl, &project.type_aliases)));
                    flow.push(Some(fl));
                }
                None => {
                    flow.push(None);
                    conc.push(None);
                }
            }
        }
        Facts { flow, conc }
    }

    pub fn flow(&self, f: FnId) -> Option<&DataFlow> {
        self.flow[f.index()].as_ref()
    }

    pub fn concurrency(&self, f: FnId) -> Option<&Concurrency> {
        self.conc[f.index()].as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct CallEdge {
    pub caller: FnId,
    pub node: NodeId,
    /// Index into the node's `calls`.
    pub call: usize,
    pub callee: FnId,
}

pub struct CallGraph {
    pub edges: Vec<CallEdge>,
    by_caller: Vec<Vec<usize>>,
    by_site: HashMap<(FnId, NodeId, usize), FnId>,
}

impl CallGraph {
    pub fn new(project: &Project) -> Self {
        let r = Resolver::new(project);
        let mut edges = Vec::new();
        let mut by_caller = vec![Vec::new(); project.functions.len()];
        let mut by_site = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            let caller = FnId(i as u32);
            let Some(body) = &f.body else { continue };
            for (n, node) in body.nodes.iter().enumerate() {
                for (ci, call) in node.calls.iter().enumerate() {
                    if let Some(callee) = r.resolve(caller, call) {
                        let node = NodeId(n as u32);
                        by_caller[i].push(edges.len());
                        by_site.insert((caller, node, ci), callee);
                        edges.push(CallEdge {
                            caller,
                            node,
                            call: ci,
                            callee,
                        });
                    }
                }
            }
        }
        CallGraph {
            edges,
            by_caller,
            by_site,
        }
    }

    pub fn calls_from(&self, f: FnId) -> impl Iterator<Item = &CallEdge> {
        self.by_caller[f.index()].iter().map(|&i| &self.edges[i])
    }

    pub fn callee(&self, f: FnId, node: NodeId, call: usize) -> Option<FnId> {
        self.by_site.get(&(f, node, call)).copied()
    }
}

/// Resolves call sites to project functions.
struct Resolver<'p> {
    project: &'p Project,
    by_name: HashMap<&'p str, Vec<FnId>>,
}

impl<'p> Resolver<'p> {
    fn new(project: &'p Project) -> Self {
        let mut by_name: HashMap<&str, Vec<FnId>> = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            by_name
                .entry(f.name.as_str())
                .or_default()
                .push(FnId(i as u32));
        }
        Resolver { project, by_name }
    }

    fn crate_of(&self, f: FnId) -> &str {
        let q = &self.project.function(f).qual_path;
        q.split("::").next().unwrap_or("")
    }

    fn resolve(&self, caller: FnId, call: &CallSite) -> Option<FnId> {
        let p = self.project;
        let caller_fn = p.function(caller);
        let cands = self.by_name.get(call.name())?;
        let same_crate = |c: &&FnId| self.crate_of(**c) == self.crate_of(caller);
        match call.kind {
            CallKind::Macro => None,
            CallKind::Function => {
                let mut path: Vec<&str> = call.path.iter().map(String::as_str).collect();
                while matches!(path.first(), Some(&"crate" | &"self" | &"super")) {
                    path.remove(0);
                }
                let self_ty = self_type(&caller_fn.owner);
                if path.first() == Some(&"Self") {
                    path[0] = self_ty?;
                }
                if path.len() == 1 {
                    let nested = cands
                        .iter()
                        .find(|c| matches!(p.function(**c).owner, FnOwner::Nested { parent } if parent == caller));
                    let same_module = cands.iter().find(|c| {
                        let f = p.function(**c);
                        f.module == caller_fn.module && f.owner == FnOwner::Free
                    });
                    let free_in_crate: Vec<&FnId> = cands
                        .iter()
                        .filter(|c| p.function(**c).owner == FnOwner::Free)
                        .filter(same_crate)
                        .collect();
                    nested
                        .or(same_module)
                        .or((free_in_crate.len() == 1).then(|| free_in_crate[0]))
                        .copied()
                } else {
                    let full = path.join("::");
                    let suffix = format!("::{full}");
                    let matching: Vec<&FnId> = cands
                        .iter()
                        .filter(|c| {
                            let q = &p.function(**c).qual_path;
                            // `mtd::x::f` from a binary calling into its library.
                            *q == full || q.ends_with(&suffix)
                        })
                        .collect();
                    matching
                        .iter()
                        .find(|c| same_crate(c))
                        .or(matching.first())
                        .map(|c| **c)
                }
            }
            CallKind::Method => {
                let methods: Vec<&FnId> = cands
                    .iter()
                    .filter(|c| {
                        matches!(
                            p.function(**c).owner,
                            FnOwner::Impl { .. } | FnOwner::Trait { .. }
                        )
                    })
                    .collect();
                if call.receiver.as_deref() == Some("self") {
                    let own = self_type(&caller_fn.owner);
                    return methods
                        .iter()
                        .find(|c| own.is_some() && self_type(&p.function(***c).owner) == own)
                        .map(|c| **c);
                }
                // A type named in the chain (`Faden::neu(..).laufen()`).
                let typed: Vec<&&FnId> = methods
                    .iter()
                    .filter(|c| {
                        self_type(&p.function(***c).owner)
                            .is_some_and(|t| call.path.iter().any(|s| s == t))
                    })
                    .collect();
                if typed.len() == 1 {
                    return Some(**typed[0]);
                }
                (methods.len() == 1).then(|| *methods[0])
            }
        }
    }
}

/// `App` for methods of `impl App` / `impl<T> Trait for App<T>`.
fn self_type(owner: &FnOwner) -> Option<&str> {
    match owner {
        FnOwner::Impl { self_ty, .. } => {
            let base = self_ty.split('<').next().unwrap_or(self_ty).trim();
            Some(base.rsplit("::").next().unwrap_or(base))
        }
        _ => None,
    }
}

/// Traits whose methods are not callbacks of an event loop.
const NOT_CALLBACK_TRAITS: &[&str] = &[
    "Clone",
    "Copy",
    "Debug",
    "Display",
    "Default",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Hash",
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "AsRef",
    "AsMut",
    "Borrow",
    "BorrowMut",
    "Deref",
    "DerefMut",
    "Serialize",
    "Deserialize",
    "Error",
    "Iterator",
    "IntoIterator",
    "FromIterator",
    "Extend",
    "FromStr",
    "ToString",
    "Drop",
    "Wake",
    "Future",
    "Stream",
    "AsRawFd",
    "AsFd",
    "AsRawSocket",
    "AsRawHandle",
    "Read",
    "Write",
    "Seek",
    "BufRead",
    "Index",
    "IndexMut",
    "Add",
    "Sub",
    "Mul",
    "Div",
    "Neg",
    "Not",
    "Sum",
    "Product",
    "Send",
    "Sync",
    "Any",
];

// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Thread {
    /// `None` for the program's main thread.
    pub kind: Option<SpawnKind>,
    pub name: Option<String>,
    pub spawned_by: Vec<usize>,
    /// Function and node of the spawn call.
    pub site: Option<(FnId, NodeId)>,
    pub in_loop: bool,
    /// Functions the thread starts with: the entry function for the main
    /// thread, the functions called directly from the lane for others.
    pub entries: Vec<FnId>,
    /// Functions executed by this thread, in discovery order.
    pub runs: Vec<FnId>,
    /// Trait-impl methods assumed to be called back by a framework on this thread.
    pub callbacks: Vec<FnId>,
}

impl Thread {
    pub fn title(&self) -> String {
        let mut t = match (self.kind, &self.name) {
            (None, _) => "main thread".to_string(),
            (Some(k), Some(n)) => format!("{} '{n}'", k.label()),
            (Some(k), None) => k.label().to_string(),
        };
        if self.in_loop {
            t.push_str(" ×n");
        }
        t
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Spawn,
    Channel { name: String, api: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: usize,
    pub to: usize,
    pub kind: LinkKind,
    /// Where the spawn happens / the channel is created.
    pub site: (FnId, NodeId),
}

/// Shared state used by more than one thread.
#[derive(Clone, Debug)]
pub struct Resource {
    pub name: String,
    pub kind: String,
    pub site: (FnId, NodeId),
    pub users: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub threads: Vec<Thread>,
    pub links: Vec<Link>,
    pub resources: Vec<Resource>,
    /// Spawn sites in functions the walk did not reach from the entry.
    pub unreached: Vec<(FnId, NodeId)>,
    /// How threads communicate, in detail.
    pub channels: Vec<ChannelComm>,
    pub shared: Vec<SharedComm>,
    pub joins: Vec<JoinComm>,
}

/// A place where a thread uses a channel end, shared object or join handle.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub thread: usize,
    pub func: FnId,
    pub node: NodeId,
    /// Method applied (`send`, `recv`, `lock`, `load`, `join`, ...); `None`
    /// if the value is only held or handed on in a way we cannot follow.
    pub op: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ChannelComm {
    pub name: String,
    /// Constructor, e.g. `std::sync::mpsc::sync_channel`.
    pub api: String,
    /// Message type if written somewhere (turbofish or parameter types).
    pub message: Option<String>,
    /// Capacity argument of bounded channels as written.
    pub capacity: Option<String>,
    pub site: (FnId, NodeId),
    pub senders: Vec<Site>,
    pub receivers: Vec<Site>,
}

#[derive(Clone, Debug)]
pub struct SharedComm {
    pub name: String,
    pub kind: String,
    pub site: (FnId, NodeId),
    pub users: Vec<Site>,
}

/// Threads waiting for another thread to finish (`handle.join()`).
#[derive(Clone, Debug)]
pub struct JoinComm {
    /// The thread that is waited for.
    pub thread: usize,
    pub waiters: Vec<Site>,
}

impl SharedComm {
    /// Threads using the object.
    pub fn threads(&self) -> BTreeSet<usize> {
        self.users.iter().map(|s| s.thread).collect()
    }
}

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
    let mut w = Walker {
        project,
        facts,
        graph,
        threads: vec![Thread {
            kind: None,
            name: None,
            spawned_by: Vec::new(),
            site: None,
            in_loop: false,
            entries: vec![entry],
            runs: Vec::new(),
            callbacks: Vec::new(),
        }],
        links: Vec::new(),
        executing: HashMap::new(),
        spawn_thread: HashMap::new(),
    };
    w.walk(entry);
    let mut program = Program::default();
    w.connect(&mut program);
    let mut links = std::mem::take(&mut w.links);
    links.append(&mut program.links);
    program.links = links;
    program.threads = std::mem::take(&mut w.threads);

    // Spawns the walk did not reach (in the entry's crate; other binaries
    // have their own entry points).
    let crate_of = |f: FnId| {
        project
            .function(f)
            .qual_path
            .split("::")
            .next()
            .unwrap_or("")
            .to_string()
    };
    let entry_crate = crate_of(entry);
    for (i, f) in project.functions.iter().enumerate() {
        let id = FnId(i as u32);
        if crate_of(id) != entry_crate {
            continue;
        }
        let Some(c) = facts.concurrency(id) else {
            continue;
        };
        for (si, s) in c.spawns.iter().enumerate() {
            if s.kind.is_parallel() && !w.spawn_thread.contains_key(&(id, si)) && f.body.is_some() {
                program.unreached.push((id, s.node));
            }
        }
    }
    program
}

struct Walker<'a> {
    project: &'a Project,
    facts: &'a Facts,
    graph: &'a CallGraph,
    threads: Vec<Thread>,
    links: Vec<Link>,
    /// Threads executing a function's own code (outside its spawned lanes).
    executing: HashMap<FnId, BTreeSet<usize>>,
    /// Thread created by a spawn site (function, spawn index).
    spawn_thread: HashMap<(FnId, usize), usize>,
}

impl Walker<'_> {
    fn body(&self, f: FnId) -> Option<&Body> {
        self.project.function(f).body.as_ref()
    }

    /// Parallel lane of `f` that executes `node` (the innermost), or `None`
    /// for the function's own code. The statement of a lane-less (inline)
    /// spawn counts as that lane: its closure runs there.
    fn region(&self, f: FnId, node: NodeId) -> Option<usize> {
        let c = self.facts.concurrency(f)?;
        let inline = c
            .spawns
            .iter()
            .position(|s| s.node == node && s.lanes.is_empty() && s.kind.is_parallel());
        inline.or_else(|| self.lane_region(f, node))
    }

    /// Innermost parallel lane (closure / async block) containing `node`.
    /// This is where a spawn call itself is executed.
    fn lane_region(&self, f: FnId, node: NodeId) -> Option<usize> {
        let (Some(body), Some(c)) = (self.body(f), self.facts.concurrency(f)) else {
            return None;
        };
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(si) = c
                .spawns
                .iter()
                .position(|s| s.kind.is_parallel() && s.lanes.contains(&n))
            {
                return Some(si);
            }
            cur = body.node(n).parent;
        }
        None
    }

    fn walk(&mut self, entry: FnId) {
        let mut queue = VecDeque::from([(entry, None, 0usize)]);
        let mut seen = HashSet::new();
        while let Some((f, region, t)) = queue.pop_front() {
            if !seen.insert((f, region, t)) {
                continue;
            }
            if region.is_none() {
                self.executing.entry(f).or_default().insert(t);
                if !self.threads[t].runs.contains(&f) {
                    self.threads[t].runs.push(f);
                }
                self.callbacks(f, t, &mut queue);
            }
            let Some(c) = self.facts.concurrency(f) else {
                continue;
            };
            // Spawns executed here start (or reuse) a thread.
            for (si, s) in c.spawns.iter().enumerate() {
                if !s.kind.is_parallel() || self.lane_region(f, s.node) != region {
                    continue;
                }
                let child = match self.spawn_thread.get(&(f, si)) {
                    Some(&child) => child,
                    None => {
                        self.threads.push(Thread {
                            kind: Some(s.kind),
                            name: s.name.clone(),
                            spawned_by: Vec::new(),
                            site: Some((f, s.node)),
                            in_loop: s.in_loop,
                            entries: Vec::new(),
                            runs: Vec::new(),
                            callbacks: Vec::new(),
                        });
                        let child = self.threads.len() - 1;
                        self.spawn_thread.insert((f, si), child);
                        child
                    }
                };
                if !self.threads[child].spawned_by.contains(&t) {
                    self.threads[child].spawned_by.push(t);
                    self.links.push(Link {
                        from: t,
                        to: child,
                        kind: LinkKind::Spawn,
                        site: (f, s.node),
                    });
                }
                queue.push_back((f, Some(si), child));
            }
            // Calls made here run in the same thread.
            let calls: Vec<(NodeId, FnId)> = self
                .graph
                .calls_from(f)
                .map(|e| (e.node, e.callee))
                .collect();
            for (node, callee) in calls {
                if self.region(f, node) == region {
                    // Calls straight from a lane are the thread's entry points.
                    if region.is_some() && !self.threads[t].entries.contains(&callee) {
                        self.threads[t].entries.push(callee);
                    }
                    queue.push_back((callee, None, t));
                }
            }
        }
    }

    /// Reaching a constructor of `X` makes `X`'s trait impls callbacks of the thread.
    fn callbacks(&mut self, f: FnId, t: usize, queue: &mut VecDeque<(FnId, Option<usize>, usize)>) {
        let func = self.project.function(f);
        let FnOwner::Impl { trait_: None, .. } = &func.owner else {
            return;
        };
        let is_ctor = func.sig.params.first().is_none_or(|p| p.pat != "self");
        let Some(ty) = self_type(&func.owner) else {
            return;
        };
        if !is_ctor {
            return;
        }
        for (i, m) in self.project.functions.iter().enumerate() {
            let FnOwner::Impl {
                trait_: Some(tr), ..
            } = &m.owner
            else {
                continue;
            };
            let trait_name = tr
                .split('<')
                .next()
                .unwrap_or(tr)
                .rsplit("::")
                .next()
                .unwrap_or(tr);
            if self_type(&m.owner) != Some(ty) || NOT_CALLBACK_TRAITS.contains(&trait_name) {
                continue;
            }
            let id = FnId(i as u32);
            if !self.threads[t].callbacks.contains(&id) {
                self.threads[t].callbacks.push(id);
            }
            queue.push_back((id, None, t));
        }
    }

    /// Threads that execute `node` of `f`.
    fn threads_at(&self, f: FnId, node: NodeId) -> BTreeSet<usize> {
        match self.region(f, node) {
            Some(si) => self
                .spawn_thread
                .get(&(f, si))
                .copied()
                .into_iter()
                .collect(),
            None => self.executing.get(&f).cloned().unwrap_or_default(),
        }
    }

    /// Places where the bindings `start` of `f` are used, following clones,
    /// call arguments into parameters, struct fields into the type's methods
    /// and return values into callers.
    fn follow(
        &self,
        f: FnId,
        start: &[usize],
        depth: usize,
        seen: &mut Seen,
        out: &mut Vec<Site>,
        types: &mut Vec<String>,
    ) {
        let (Some(body), Some(flow)) = (self.body(f), self.facts.flow(f)) else {
            return;
        };
        let mut bindings: HashSet<usize> = start
            .iter()
            .copied()
            .filter(|b| seen.bindings.insert((f, *b)))
            .collect();
        if bindings.is_empty() || depth > 10 {
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
                types.push(ty);
            }
        }
        let binding_at = |node: NodeId, name: &str| -> Option<usize> {
            flow.edges
                .iter()
                .find(|e| e.to == node && flow.bindings[e.binding].name == name)
                .map(|e| e.binding)
        };
        for (n, node) in body.nodes.iter().enumerate() {
            let id = NodeId(n as u32);
            let ours = |name: &str, bindings: &HashSet<usize>| {
                binding_at(id, name).is_some_and(|b| bindings.contains(&b))
            };
            if !node.uses.iter().any(|u| ours(&u.name, &bindings)) {
                continue;
            }
            // `let b = a.clone()` / `Arc::clone(&a)` keeps following.
            let is_clone = node.calls.iter().any(|c| {
                c.name() == "clone"
                    && (c.receiver.as_deref().is_some_and(|r| ours(r, &bindings))
                        || c.args
                            .first()
                            .cloned()
                            .flatten()
                            .is_some_and(|a| ours(&a, &bindings)))
            });
            if is_clone
                && node.defs.len() == 1
                && let Some(new) = flow
                    .bindings
                    .iter()
                    .rposition(|b| b.def == id && b.name == node.defs[0].name)
            {
                bindings.insert(new);
                continue;
            }
            let mut handled = false;
            // Methods called on it.
            for call in &node.calls {
                if call.receiver.as_deref().is_some_and(|r| ours(r, &bindings))
                    && call.field.is_none()
                {
                    let op = call.path.first().cloned();
                    if op.as_deref() != Some("clone") {
                        self.sites(f, id, op, out);
                        handled = true;
                    }
                }
            }
            // Handed to a function.
            for (ci, call) in node.calls.iter().enumerate() {
                let Some(callee) = self.graph.callee(f, id, ci) else {
                    continue;
                };
                for (k, arg) in call.args.iter().enumerate() {
                    if arg.as_deref().is_some_and(|a| ours(a, &bindings))
                        && let Some(param) =
                            param_binding(self.project, self.facts, callee, call.kind, k)
                    {
                        self.follow(callee, &[param], depth + 1, seen, out, types);
                        handled = true;
                    }
                }
            }
            // Stored in a struct field: used by the type's methods.
            for u in &node.uses {
                if let Some((strukt, field)) = &u.stored_in
                    && ours(&u.name, &bindings)
                {
                    let ty = if strukt == "Self" {
                        self_type(&self.project.function(f).owner).map(str::to_string)
                    } else {
                        Some(strukt.clone())
                    };
                    if let Some(ty) = ty {
                        // The storing thread holds it, the type's methods use it.
                        self.sites(f, id, Some(format!("stored in {ty}.{field}")), out);
                        self.follow_field(&ty, field, depth + 1, seen, out, types);
                        handled = true;
                    }
                }
            }
            // Returned: continue where the caller binds the result.
            if is_returned(body, id) {
                let names: Vec<&str> = node.uses.iter().map(|u| u.name.as_str()).collect();
                let k = names.iter().position(|nm| ours(nm, &bindings)).unwrap_or(0);
                for e in self.graph.edges.iter().filter(|e| e.callee == f) {
                    let (Some(cbody), Some(cflow)) =
                        (self.body(e.caller), self.facts.flow(e.caller))
                    else {
                        continue;
                    };
                    let defs = &cbody.node(e.node).defs;
                    let def = if defs.len() == 1 {
                        defs.first()
                    } else {
                        defs.get(k)
                    };
                    if let Some(d) = def
                        && let Some(b) = cflow
                            .bindings
                            .iter()
                            .rposition(|b| b.def == e.node && b.name == d.name)
                    {
                        self.follow(e.caller, &[b], depth + 1, seen, out, types);
                        handled = true;
                    }
                }
            }
            if !handled {
                self.sites(f, id, None, out);
            }
        }
    }

    /// Uses of `self.<field>` in the methods of `ty`.
    fn follow_field(
        &self,
        ty: &str,
        field: &str,
        depth: usize,
        seen: &mut Seen,
        out: &mut Vec<Site>,
        types: &mut Vec<String>,
    ) {
        if !seen.fields.insert((ty.to_string(), field.to_string())) {
            return;
        }
        for (i, m) in self.project.functions.iter().enumerate() {
            if self_type(&m.owner) != Some(ty) {
                continue;
            }
            let id = FnId(i as u32);
            let (Some(body), Some(flow)) = (m.body.as_ref(), self.facts.flow(id)) else {
                continue;
            };
            for (n, node) in body.nodes.iter().enumerate() {
                let nid = NodeId(n as u32);
                for call in &node.calls {
                    if call.receiver.as_deref() != Some("self")
                        || call.field.as_deref() != Some(field)
                    {
                        continue;
                    }
                    let op = call.path.first().cloned();
                    if op.as_deref() == Some("clone") {
                        // `let g = self.field.clone()` (e.g. before spawning).
                        if node.defs.len() == 1
                            && let Some(b) = flow
                                .bindings
                                .iter()
                                .rposition(|b| b.def == nid && b.name == node.defs[0].name)
                        {
                            self.follow(id, &[b], depth + 1, seen, out, types);
                        }
                    } else {
                        self.sites(id, nid, op, out);
                    }
                }
            }
        }
    }

    /// One site per thread executing `node` of `f`.
    fn sites(&self, f: FnId, node: NodeId, op: Option<String>, out: &mut Vec<Site>) {
        for thread in self.threads_at(f, node) {
            let site = Site {
                thread,
                func: f,
                node,
                op: op.clone(),
            };
            if !out.contains(&site) {
                out.push(site);
            }
        }
    }

    fn connect(&self, program: &mut Program) {
        for (i, f) in self.project.functions.iter().enumerate() {
            let id = FnId(i as u32);
            let (Some(c), Some(body)) = (self.facts.concurrency(id), f.body.as_ref()) else {
                continue;
            };
            let creators = |node: NodeId| self.threads_at(id, node);
            // Channels created here.
            for ch in &c.channels {
                if creators(ch.def).is_empty() {
                    continue; // function not reached from this entry
                }
                let mut types = Vec::new();
                let mut senders = Vec::new();
                let mut receivers = Vec::new();
                self.follow(
                    id,
                    &ch.senders,
                    0,
                    &mut Seen::default(),
                    &mut senders,
                    &mut types,
                );
                self.follow(
                    id,
                    &ch.receivers,
                    0,
                    &mut Seen::default(),
                    &mut receivers,
                    &mut types,
                );
                for list in [&mut senders, &mut receivers] {
                    if list.is_empty() {
                        self.sites(id, ch.def, None, list);
                    }
                }
                let ctor = body
                    .node(ch.def)
                    .calls
                    .iter()
                    .find(|c| ch.api == c.path.join("::"));
                let message = ctor
                    .and_then(|c| c.turbofish.clone())
                    .or_else(|| types.iter().find_map(|t| generic_arg(t)));
                let capacity = ctor.and_then(|c| {
                    (matches!(c.name(), "sync_channel" | "bounded" | "channel")
                        && !c.arg_texts.is_empty())
                    .then(|| c.arg_texts[0].clone())
                });
                let s_threads: BTreeSet<usize> = senders.iter().map(|s| s.thread).collect();
                let r_threads: BTreeSet<usize> = receivers.iter().map(|s| s.thread).collect();
                for &s in &s_threads {
                    for &r in &r_threads {
                        if s != r {
                            program.links.push(Link {
                                from: s,
                                to: r,
                                kind: LinkKind::Channel {
                                    name: ch.name.clone(),
                                    api: ch.api.clone(),
                                },
                                site: (id, ch.def),
                            });
                        }
                    }
                }
                program.channels.push(ChannelComm {
                    name: ch.name.clone(),
                    api: ch.api.clone(),
                    message,
                    capacity,
                    site: (id, ch.def),
                    senders,
                    receivers,
                });
            }
            // Shared state created here (`Arc::new`).
            for g in &c.shared {
                let created = body
                    .node(g.def)
                    .calls
                    .iter()
                    .any(|c| c.ends_with(&["Arc", "new"]));
                if !created || creators(g.def).is_empty() {
                    continue;
                }
                let mut users = Vec::new();
                self.follow(
                    id,
                    &g.bindings,
                    0,
                    &mut Seen::default(),
                    &mut users,
                    &mut Vec::new(),
                );
                let shared = SharedComm {
                    name: g.name.clone(),
                    kind: g.kind.clone(),
                    site: (id, g.def),
                    users,
                };
                let threads = shared.threads();
                if threads.len() >= 2 {
                    program.resources.push(Resource {
                        name: g.name.clone(),
                        kind: g.kind.clone(),
                        site: (id, g.def),
                        users: threads.into_iter().collect(),
                    });
                    program.shared.push(shared);
                }
            }
            // Join handles of threads started here.
            for (si, s) in c.spawns.iter().enumerate() {
                let Some(&thread) = self.spawn_thread.get(&(id, si)) else {
                    continue;
                };
                let node = body.node(s.node);
                if node.defs.len() != 1 {
                    continue;
                }
                let Some(flow) = self.facts.flow(id) else {
                    continue;
                };
                let Some(handle) = flow
                    .bindings
                    .iter()
                    .rposition(|b| b.def == s.node && b.name == node.defs[0].name)
                else {
                    continue;
                };
                let mut sites = Vec::new();
                self.follow(
                    id,
                    &[handle],
                    0,
                    &mut Seen::default(),
                    &mut sites,
                    &mut Vec::new(),
                );
                let waiters: Vec<Site> = sites
                    .into_iter()
                    .filter(|s| s.op.as_deref() == Some("join"))
                    .collect();
                if !waiters.is_empty() {
                    program.joins.push(JoinComm { thread, waiters });
                }
            }
        }
    }
}

#[derive(Default)]
struct Seen {
    bindings: HashSet<(FnId, usize)>,
    fields: HashSet<(String, String)>,
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

/// `T` in `Sender<T>`, `Receiver<T>`, `SyncSender<T>`, `mpsc::Receiver<T>`.
fn generic_arg(ty: &str) -> Option<String> {
    let ty = ty.trim_start_matches('&').trim_start_matches("mut ").trim();
    let head = ty.split('<').next()?;
    let last = head.rsplit("::").next()?;
    if !matches!(
        last,
        "Sender" | "SyncSender" | "Receiver" | "UnboundedSender" | "UnboundedReceiver"
    ) {
        return None;
    }
    let inner = ty.get(head.len() + 1..ty.len().checked_sub(1)?)?;
    Some(inner.trim().to_string())
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
