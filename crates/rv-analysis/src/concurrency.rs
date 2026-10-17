//! Concurrency facts of one function: what runs in parallel (lanes), which
//! channels connect them and which shared state they touch.
//!
//! Recognition is by API name (syntactic): the tables below list the paths
//! and methods that are understood.

use std::collections::{HashMap, HashSet};

use rv_ir::{Body, CallKind, CallSite, NodeId, NodeKind};

use crate::DataFlow;

/// Path segments that make a `channel()`-like call a channel constructor.
const CHANNEL_MODULES: &[&str] = &[
    "mpsc",
    "crossbeam",
    "crossbeam_channel",
    "channel",
    "oneshot",
    "broadcast",
    "watch",
    "async_channel",
    "flume",
    "kanal",
];
const CHANNEL_CTORS: &[&str] = &[
    "channel",
    "sync_channel",
    "bounded",
    "unbounded",
    "unbounded_channel",
    "oneshot",
];
const SEND_METHODS: &[&str] = &[
    "send",
    "try_send",
    "send_timeout",
    "blocking_send",
    "send_blocking",
    "send_async",
    "force_send",
];
const RECV_METHODS: &[&str] = &[
    "recv",
    "try_recv",
    "recv_timeout",
    "recv_deadline",
    "recv_async",
    "blocking_recv",
    "iter",
    "try_iter",
    "changed",
    "borrow",
    "borrow_and_update",
];
/// Async runtimes whose `spawn` starts a task.
const ASYNC_RUNTIMES: &[&str] = &[
    "tokio",
    "task",
    "smol",
    "async_std",
    "async_global_executor",
    "glommio",
    "monoio",
];
const PAR_ITER_METHODS: &[&str] = &[
    "par_iter",
    "par_iter_mut",
    "into_par_iter",
    "par_chunks",
    "par_chunks_mut",
    "par_bridge",
    "par_extend",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpawnKind {
    /// OS thread (`thread::spawn`, `thread::Builder::spawn`).
    Thread,
    /// Thread of a `thread::scope`.
    ScopedThread,
    /// Task on an async runtime (`tokio::spawn`, `smol::spawn`, `executor.spawn`).
    AsyncTask,
    /// Blocking task on a runtime's thread pool.
    Blocking,
    /// rayon `scope.spawn` / `rayon::spawn`.
    RayonTask,
    /// Data-parallel iterator (rayon `par_iter` ...).
    ParallelIter,
    /// Runs futures/closures concurrently and waits for all (`join!`, `rayon::join`, `future::zip`).
    Join,
    /// Waits for the first of several futures (`select!`, `future::race`).
    Select,
    /// Runs a future to completion on the current thread (`block_on`); not parallel.
    BlockOn,
}

impl SpawnKind {
    pub fn label(self) -> &'static str {
        match self {
            SpawnKind::Thread => "thread",
            SpawnKind::ScopedThread => "scoped thread",
            SpawnKind::AsyncTask => "async task",
            SpawnKind::Blocking => "blocking task",
            SpawnKind::RayonTask => "rayon task",
            SpawnKind::ParallelIter => "parallel iterator",
            SpawnKind::Join => "join",
            SpawnKind::Select => "select",
            SpawnKind::BlockOn => "block_on",
        }
    }

    /// Starts something that runs at the same time as the caller.
    pub fn is_parallel(self) -> bool {
        !matches!(self, SpawnKind::BlockOn)
    }
}

#[derive(Clone, Debug)]
pub struct Spawn {
    /// Node containing the call.
    pub node: NodeId,
    pub kind: SpawnKind,
    /// Thread name from `Builder::name("...")`.
    pub name: Option<String>,
    /// Closures / async blocks that run concurrently (structures in the body).
    pub lanes: Vec<NodeId>,
    /// Single-expression closures that run concurrently, as text.
    pub inline: Vec<String>,
    /// The API that was recognized, e.g. `thread::Builder::spawn`.
    pub api: String,
    /// Inside a loop: possibly many instances.
    pub in_loop: bool,
    /// Functions called in the lane (first one is usually the entry point).
    pub runs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum End {
    Send,
    Recv,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub def: NodeId,
    /// Constructor as written, e.g. `std::sync::mpsc::sync_channel`.
    pub api: String,
    /// Display name, e.g. `sample_tx → rx`.
    pub name: String,
    pub senders: Vec<usize>,
    pub receivers: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelUse {
    pub node: NodeId,
    pub channel: usize,
    pub end: End,
    /// `send`, `recv`, ... or `None` if the end is passed on (moved into a
    /// call, captured, returned).
    pub op: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SharedGroup {
    pub def: NodeId,
    /// Name of the first binding.
    pub name: String,
    /// `Mutex`, `RwLock`, `Atomic`, `Condvar` or `Arc`.
    pub kind: String,
    pub bindings: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedUse {
    pub node: NodeId,
    pub group: usize,
    /// First method applied (`lock`, `load`, ...) or `None` if passed on.
    pub op: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Concurrency {
    pub spawns: Vec<Spawn>,
    pub channels: Vec<Channel>,
    pub channel_uses: Vec<ChannelUse>,
    pub shared: Vec<SharedGroup>,
    pub shared_uses: Vec<SharedUse>,
}

impl Concurrency {
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty() && self.channels.is_empty() && self.shared.is_empty()
    }

    /// The spawn whose lane contains `node` (innermost), if any.
    pub fn lane_of(&self, body: &Body, node: NodeId) -> Option<usize> {
        let mut cur = Some(node);
        while let Some(n) = cur {
            if let Some(i) = self.spawns.iter().position(|s| s.lanes.contains(&n)) {
                return Some(i);
            }
            cur = body.node(n).parent;
        }
        None
    }
}

pub fn concurrency(body: &Body, flow: &DataFlow, aliases: &HashMap<String, String>) -> Concurrency {
    let mut c = Concurrency::default();
    let ids: Vec<NodeId> = (0..body.nodes.len() as u32).map(NodeId).collect();
    let binding_at = |node: NodeId, name: &str| -> Option<usize> {
        flow.edges
            .iter()
            .filter(|e| e.to == node && flow.bindings[e.binding].name == name)
            .map(|e| e.binding)
            .next()
    };
    let defined_at = |node: NodeId, name: &str| -> Option<usize> {
        flow.bindings
            .iter()
            .rposition(|b| b.def == node && b.name == name)
    };

    // --- scopes whose closure parameter spawns scoped threads / rayon tasks
    let mut scope_handles: HashMap<usize, SpawnKind> = HashMap::new();
    for &id in &ids {
        for call in &body.node(id).calls {
            let kind = if call.ends_with(&["thread", "scope"]) {
                SpawnKind::ScopedThread
            } else if call.ends_with(&["rayon", "scope"])
                || call.ends_with(&["rayon", "in_place_scope"])
            {
                SpawnKind::RayonTask
            } else {
                continue;
            };
            for &s in &call.structures {
                let Some(&frame) = body.node(s).children.first() else {
                    continue;
                };
                for d in &body.node(frame).defs {
                    if let Some(b) = defined_at(frame, &d.name) {
                        scope_handles.insert(b, kind);
                    }
                }
            }
        }
    }

    // --- spawns
    for &id in &ids {
        let node = body.node(id);
        for call in &node.calls {
            let Some((kind, api)) = classify_spawn(call, |r| {
                binding_at(id, r).and_then(|b| scope_handles.get(&b).copied())
            }) else {
                continue;
            };
            let mut lanes = call.structures.clone();
            if kind == SpawnKind::ParallelIter {
                // Closures of the adapters after `par_iter()` run in parallel.
                for other in &node.calls {
                    if other
                        .path
                        .iter()
                        .any(|p| PAR_ITER_METHODS.contains(&p.as_str()))
                    {
                        lanes.extend(other.structures.iter().copied());
                    }
                }
                lanes.sort();
                lanes.dedup();
            }
            let name = call
                .literals
                .iter()
                .find(|(link, _)| link == "name")
                .map(|(_, v)| v.clone());
            let mut runs = Vec::new();
            for &lane in &lanes {
                collect_calls(body, lane, &mut runs);
            }
            if lanes.is_empty() {
                // Inline closure: its calls were recorded on this node.
                runs.extend(
                    node.calls
                        .iter()
                        .filter(|c| c.kind == CallKind::Function && !std::ptr::eq(*c, call))
                        .map(|c| c.path.join("::")),
                );
            }
            runs.retain(|r| !is_plumbing(r));
            let mut seen = HashSet::new();
            runs.retain(|r| seen.insert(r.clone()));
            c.spawns.push(Spawn {
                node: id,
                kind,
                name,
                lanes,
                inline: call.inline_closures.clone(),
                api,
                in_loop: in_loop(body, id),
                runs,
            });
        }
    }

    // --- channels and their ends
    for &id in &ids {
        let node = body.node(id);
        if node.defs.len() != 2 {
            continue;
        }
        let Some(ctor) = node.calls.iter().find(|c| is_channel_ctor(c)) else {
            continue;
        };
        let (Some(tx), Some(rx)) = (
            defined_at(id, &node.defs[0].name),
            defined_at(id, &node.defs[1].name),
        ) else {
            continue;
        };
        c.channels.push(Channel {
            def: id,
            api: ctor.path.join("::"),
            name: format!("{} → {}", node.defs[0].name, node.defs[1].name),
            senders: vec![tx],
            receivers: vec![rx],
        });
    }
    // Clones of channel ends and of shared handles are aliases.
    let clone_source = |id: NodeId| -> Option<(usize, usize)> {
        let node = body.node(id);
        if node.defs.len() != 1 {
            return None;
        }
        let new = defined_at(id, &node.defs[0].name)?;
        let src_name = node.calls.iter().find_map(|c| {
            if c.kind == CallKind::Method && c.path.first().map(String::as_str) == Some("clone") {
                c.receiver.clone()
            } else if c.kind == CallKind::Function && c.name() == "clone" {
                c.arg_names.first().cloned()
            } else {
                None
            }
        })?;
        Some((binding_at(id, &src_name)?, new))
    };

    // --- shared state
    for &id in &ids {
        let node = body.node(id);
        let created = node.calls.iter().any(|c| c.ends_with(&["Arc", "new"]));
        for d in &node.defs {
            let ty_kind =
                d.ty.as_deref()
                    .and_then(|t| shared_kind_of_type(t, aliases));
            let Some(b) = defined_at(id, &d.name) else {
                continue;
            };
            let kind = if created && node.defs.len() == 1 {
                Some(shared_kind_of_calls(&node.calls))
            } else {
                ty_kind
            };
            if let Some(kind) = kind {
                c.shared.push(SharedGroup {
                    def: id,
                    name: d.name.clone(),
                    kind,
                    bindings: vec![b],
                });
            }
        }
    }
    for &id in &ids {
        if let Some((src, new)) = clone_source(id) {
            for ch in &mut c.channels {
                if ch.senders.contains(&src) {
                    ch.senders.push(new);
                }
                if ch.receivers.contains(&src) {
                    ch.receivers.push(new);
                }
            }
            for g in &mut c.shared {
                if g.bindings.contains(&src) {
                    g.bindings.push(new);
                }
            }
        }
    }

    // --- uses of channel ends and shared handles
    let mut seen_ch = HashSet::new();
    let mut seen_sh = HashSet::new();
    for &id in &ids {
        let node = body.node(id);
        let mut receivers = HashSet::new();
        for call in &node.calls {
            let Some(r) = &call.receiver else { continue };
            let Some(b) = binding_at(id, r) else { continue };
            receivers.insert(r.clone());
            let first = call.path.first().cloned().unwrap_or_default();
            if first == "clone" {
                continue;
            }
            for (ci, ch) in c.channels.iter().enumerate() {
                let end = if ch.senders.contains(&b) {
                    End::Send
                } else if ch.receivers.contains(&b) {
                    End::Recv
                } else {
                    continue;
                };
                let known = match end {
                    End::Send => SEND_METHODS.contains(&first.as_str()),
                    End::Recv => RECV_METHODS.contains(&first.as_str()),
                };
                let op = Some(if known {
                    first.clone()
                } else {
                    format!("{first}()")
                });
                if seen_ch.insert((id, ci, end)) {
                    c.channel_uses.push(ChannelUse {
                        node: id,
                        channel: ci,
                        end,
                        op,
                    });
                }
            }
            for (gi, g) in c.shared.iter().enumerate() {
                if g.bindings.contains(&b) && seen_sh.insert((id, gi)) {
                    c.shared_uses.push(SharedUse {
                        node: id,
                        group: gi,
                        op: Some(first.clone()),
                    });
                }
            }
        }
        // Passed on: used, but not as the receiver of a call.
        for u in &node.uses {
            if receivers.contains(&u.name) {
                continue;
            }
            let Some(b) = binding_at(id, &u.name) else {
                continue;
            };
            for (ci, ch) in c.channels.iter().enumerate() {
                if ch.def == id {
                    continue;
                }
                let end = if ch.senders.contains(&b) {
                    End::Send
                } else if ch.receivers.contains(&b) {
                    End::Recv
                } else {
                    continue;
                };
                if seen_ch.insert((id, ci, end)) {
                    c.channel_uses.push(ChannelUse {
                        node: id,
                        channel: ci,
                        end,
                        op: None,
                    });
                }
            }
            for (gi, g) in c.shared.iter().enumerate() {
                if g.bindings.contains(&b)
                    && clone_source(id).is_none_or(|(src, _)| src != b)
                    && seen_sh.insert((id, gi))
                {
                    c.shared_uses.push(SharedUse {
                        node: id,
                        group: gi,
                        op: None,
                    });
                }
            }
        }
    }
    c
}

/// Recognize a call that starts concurrent work.
fn classify_spawn(
    call: &CallSite,
    scope_kind: impl Fn(&str) -> Option<SpawnKind>,
) -> Option<(SpawnKind, String)> {
    let name = call.name();
    let has = |seg: &str| call.path.iter().any(|p| p == seg);
    let api = || call.path.join("::");
    match call.kind {
        CallKind::Function => {
            if call.ends_with(&["thread", "spawn"]) {
                Some((SpawnKind::Thread, "thread::spawn".into()))
            } else if name == "spawn_blocking" {
                Some((SpawnKind::Blocking, api()))
            } else if call.ends_with(&["rayon", "spawn"]) {
                Some((SpawnKind::RayonTask, api()))
            } else if (name == "spawn" || name == "spawn_local")
                && ASYNC_RUNTIMES.iter().any(|r| has(r))
            {
                Some((SpawnKind::AsyncTask, api()))
            } else if name == "block_on" {
                Some((SpawnKind::BlockOn, api()))
            } else if call.ends_with(&["rayon", "join"])
                || call.ends_with(&["future", "zip"])
                || call.ends_with(&["future", "try_zip"])
            {
                Some((SpawnKind::Join, api()))
            } else if call.ends_with(&["future", "race"]) || call.ends_with(&["future", "or"]) {
                Some((SpawnKind::Select, api()))
            } else {
                None
            }
        }
        CallKind::Method => {
            if (name == "spawn" || name == "spawn_scoped")
                && (call.arg_count == 0 || has("Command"))
            {
                // `std::process::Command::spawn()` starts a process, not a task.
                None
            } else if name == "spawn" || name == "spawn_scoped" {
                if has("Builder") {
                    Some((SpawnKind::Thread, "thread::Builder::spawn".into()))
                } else if let Some(kind) = call.receiver.as_deref().and_then(&scope_kind) {
                    Some((
                        kind,
                        format!("{}.spawn", call.receiver.as_deref().unwrap_or("scope")),
                    ))
                } else {
                    Some((
                        SpawnKind::AsyncTask,
                        format!("{}.spawn", call.receiver.as_deref().unwrap_or("executor")),
                    ))
                }
            } else if name == "spawn_blocking" {
                Some((SpawnKind::Blocking, api()))
            } else if name == "block_on" {
                Some((
                    SpawnKind::BlockOn,
                    format!("{}.block_on", call.receiver.as_deref().unwrap_or("runtime")),
                ))
            } else if PAR_ITER_METHODS.contains(&name) {
                Some((SpawnKind::ParallelIter, name.to_string()))
            } else {
                None
            }
        }
        CallKind::Macro => match name {
            "join" | "try_join" => Some((SpawnKind::Join, format!("{}!", api()))),
            "select" | "select_biased" => Some((SpawnKind::Select, format!("{}!", api()))),
            _ => None,
        },
    }
}

fn is_channel_ctor(call: &CallSite) -> bool {
    if call.kind != CallKind::Function || !CHANNEL_CTORS.contains(&call.name()) {
        return false;
    }
    call.path.len() == 1
        || call
            .path
            .iter()
            .any(|p| CHANNEL_MODULES.contains(&p.as_str()))
}

fn shared_kind_of_calls(calls: &[CallSite]) -> String {
    for call in calls {
        for seg in &call.path {
            for kind in ["Mutex", "RwLock", "Condvar"] {
                if seg == kind {
                    return kind.into();
                }
            }
            if seg.starts_with("Atomic") {
                return "Atomic".into();
            }
        }
    }
    "Arc".into()
}

/// `Arc<Mutex<T>>`, `&AtomicBool`, or an alias of such a type.
fn shared_kind_of_type(ty: &str, aliases: &HashMap<String, String>) -> Option<String> {
    let base = ty.trim_start_matches('&').trim_start_matches("mut ").trim();
    let head = base
        .split('<')
        .next()
        .unwrap_or(base)
        .rsplit("::")
        .next()
        .unwrap_or(base);
    let expanded = aliases.get(head).map_or(base, String::as_str);
    for kind in ["Mutex", "RwLock", "Condvar"] {
        if expanded.contains(&format!("{kind}<")) || expanded.ends_with(kind) {
            return Some(kind.into());
        }
    }
    if expanded.contains("Atomic") {
        return Some("Atomic".into());
    }
    expanded.contains("Arc<").then(|| "Arc".into())
}

fn in_loop(body: &Body, node: NodeId) -> bool {
    let mut cur = body.node(node).parent;
    while let Some(n) = cur {
        if let NodeKind::Structure { kind, .. } = &body.node(n).kind {
            if kind.is_loop() {
                return true;
            }
            if kind.is_fn_like() {
                return false;
            }
        }
        cur = body.node(n).parent;
    }
    false
}

/// Function calls inside a lane, in source order.
fn collect_calls(body: &Body, id: NodeId, out: &mut Vec<String>) {
    let node = body.node(id);
    for call in &node.calls {
        if call.kind == CallKind::Function {
            out.push(call.path.join("::"));
        }
    }
    for &c in &node.children {
        collect_calls(body, c, out);
    }
}

/// Calls that say nothing about what a lane does: everything except free
/// functions (`work`, `module::work`), e.g. constructors (`Evt::Error`) and
/// associated functions of types (`Duration::from_millis`).
fn is_plumbing(path: &str) -> bool {
    let segments: Vec<&str> = path.split("::").collect();
    let lower = |s: &str| {
        s.chars()
            .next()
            .is_some_and(|c| c.is_lowercase() || c == '_')
    };
    !segments.iter().all(|s| lower(s))
        || matches!(segments[0], "std" | "core" | "alloc" | "log")
        || segments.last() == Some(&"drop")
}
