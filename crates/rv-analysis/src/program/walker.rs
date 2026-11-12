//! Walks the call graph from an entry point and finds out which thread runs
//! which function: every reached spawn starts a thread, calls stay in the
//! thread of their caller.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use rv_ir::{Body, FnId, FnOwner, NodeId, Project};

use super::callgraph::CallGraph;
use super::facts::Facts;
use super::model::{Link, LinkKind, Site, Thread};
use super::resolver::{crate_name, self_type};
use crate::concurrency::Spawn;

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

/// Code waiting to be walked: a function, the parallel lane of it that is
/// meant (`None` for the function's own code) and the thread executing it.
type Work = (FnId, Option<usize>, usize);

pub(super) struct Walker<'a> {
    pub(super) project: &'a Project,
    pub(super) facts: &'a Facts,
    pub(super) graph: &'a CallGraph,
    pub(super) threads: Vec<Thread>,
    pub(super) links: Vec<Link>,
    /// Threads executing a function's own code (outside its spawned lanes).
    executing: HashMap<FnId, BTreeSet<usize>>,
    /// Thread created by a spawn site (function, spawn index).
    pub(super) spawn_thread: HashMap<(FnId, usize), usize>,
}

impl<'a> Walker<'a> {
    /// A walker that knows only the main thread, which starts with `entry`.
    pub(super) fn new(
        project: &'a Project,
        facts: &'a Facts,
        graph: &'a CallGraph,
        entry: FnId,
    ) -> Self {
        Walker {
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
        }
    }

    pub(super) fn body(&self, f: FnId) -> Option<&'a Body> {
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

    pub(super) fn walk(&mut self, entry: FnId) {
        let facts = self.facts;
        let mut queue: VecDeque<Work> = VecDeque::from([(entry, None, 0usize)]);
        let mut seen = HashSet::new();
        while let Some(work) = queue.pop_front() {
            if !seen.insert(work) {
                continue;
            }
            let (f, region, t) = work;
            if region.is_none() {
                self.executing.entry(f).or_default().insert(t);
                if !self.threads[t].runs.contains(&f) {
                    self.threads[t].runs.push(f);
                }
                self.callbacks(f, t, &mut queue);
            }
            let Some(c) = facts.concurrency(f) else {
                continue;
            };
            // Spawns executed here start (or reuse) a thread.
            for (si, s) in c.spawns.iter().enumerate() {
                if s.kind.is_parallel() && self.lane_region(f, s.node) == region {
                    let child = self.spawn(t, f, si, s);
                    queue.push_back((f, Some(si), child));
                }
            }
            self.calls(work, &mut queue);
        }
    }

    /// Thread `t` executes spawn `si` of `f`: the thread it starts (a new
    /// one the first time the spawn is reached), linked to `t`.
    fn spawn(&mut self, t: usize, f: FnId, si: usize, s: &Spawn) -> usize {
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
        child
    }

    /// Calls made in the code of `work` run in the same thread.
    fn calls(&mut self, (f, region, t): Work, queue: &mut VecDeque<Work>) {
        let graph = self.graph;
        for e in graph.calls_from(f) {
            if self.region(f, e.node) != region {
                continue;
            }
            // Calls straight from a lane are the thread's entry points.
            if region.is_some() && !self.threads[t].entries.contains(&e.callee) {
                self.threads[t].entries.push(e.callee);
            }
            queue.push_back((e.callee, None, t));
        }
    }

    /// Reaching a constructor of `X` makes `X`'s trait impls callbacks of the thread.
    fn callbacks(&mut self, f: FnId, t: usize, queue: &mut VecDeque<Work>) {
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
        // Types of different binaries may share a name (`App`).
        let own_crate = crate_name(&func.qual_path);
        for (i, m) in self.project.functions.iter().enumerate() {
            if crate_name(&m.qual_path) != own_crate {
                continue;
            }
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
    pub(super) fn threads_at(&self, f: FnId, node: NodeId) -> BTreeSet<usize> {
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

    /// One site per thread executing `node` of `f`.
    pub(super) fn sites(&self, f: FnId, node: NodeId, op: Option<String>, out: &mut Vec<Site>) {
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

    /// Parallel spawns the walk did not reach, in the crate of `entry`
    /// (other binaries have their own entry points).
    pub(super) fn unreached(&self, entry: FnId) -> Vec<(FnId, NodeId)> {
        let crate_of = |f: FnId| crate_name(&self.project.function(f).qual_path);
        let entry_crate = crate_of(entry);
        let mut unreached = Vec::new();
        for (i, f) in self.project.functions.iter().enumerate() {
            let id = FnId(i as u32);
            if crate_of(id) != entry_crate {
                continue;
            }
            let Some(c) = self.facts.concurrency(id) else {
                continue;
            };
            for (si, s) in c.spawns.iter().enumerate() {
                if s.kind.is_parallel()
                    && !self.spawn_thread.contains_key(&(id, si))
                    && f.body.is_some()
                {
                    unreached.push((id, s.node));
                }
            }
        }
        unreached
    }
}
