//! Calls that start concurrent work: threads, tasks, parallel iterators,
//! `join!` / `select!` — and what runs in their lanes.

use std::collections::{HashMap, HashSet};

use rv_ir::{Body, CallKind, CallSite, Node, NodeId, NodeKind};

use super::{Spawn, SpawnKind};
use crate::DataFlow;

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

/// Free functions that run closures or futures concurrently: the last two
/// path segments and what they do.
const COMBINATORS: &[([&str; 2], SpawnKind)] = &[
    (["rayon", "spawn"], SpawnKind::RayonTask),
    (["rayon", "join"], SpawnKind::Join),
    (["future", "zip"], SpawnKind::Join),
    (["future", "try_zip"], SpawnKind::Join),
    (["future", "race"], SpawnKind::Select),
    (["future", "or"], SpawnKind::Select),
];

/// The spawns of a body, in source order.
pub(super) fn spawns(body: &Body, flow: &DataFlow) -> Vec<Spawn> {
    let scope_handles = scope_handles(body, flow);
    let mut spawns = Vec::new();
    for (n, node) in body.nodes.iter().enumerate() {
        let id = NodeId(n as u32);
        for call in &node.calls {
            let scope_kind = |receiver: &str| {
                flow.binding_at(id, receiver)
                    .and_then(|b| scope_handles.get(&b).copied())
            };
            let Some((kind, api)) = classify_spawn(call, scope_kind) else {
                continue;
            };
            let lanes = lanes(node, call, kind);
            let runs = lane_calls(body, node, call, &lanes);
            spawns.push(Spawn {
                node: id,
                kind,
                name: thread_name(call),
                lanes,
                inline: call.inline_closures.clone(),
                api,
                in_loop: in_loop(body, id),
                runs,
            });
        }
    }
    spawns
}

/// Bindings of the closure parameters of `thread::scope(|s| …)` /
/// `rayon::scope(|s| …)`: calling `spawn` on them starts scoped threads /
/// rayon tasks.
fn scope_handles(body: &Body, flow: &DataFlow) -> HashMap<usize, SpawnKind> {
    let mut handles = HashMap::new();
    for call in body.nodes.iter().flat_map(|n| &n.calls) {
        let Some(kind) = scope_kind(call) else {
            continue;
        };
        for &closure in &call.structures {
            for param in closure_params(body, flow, closure) {
                handles.insert(param, kind);
            }
        }
    }
    handles
}

/// What `spawn` on the scope handle of this call starts, if it opens a scope.
fn scope_kind(call: &CallSite) -> Option<SpawnKind> {
    if call.ends_with(&["thread", "scope"]) {
        Some(SpawnKind::ScopedThread)
    } else if call.ends_with(&["rayon", "scope"]) || call.ends_with(&["rayon", "in_place_scope"]) {
        Some(SpawnKind::RayonTask)
    } else {
        None
    }
}

/// Bindings defined by the first frame of a closure structure: its parameters.
fn closure_params(body: &Body, flow: &DataFlow, closure: NodeId) -> Vec<usize> {
    let Some(&frame) = body.node(closure).children.first() else {
        return Vec::new();
    };
    body.node(frame)
        .defs
        .iter()
        .filter_map(|d| flow.defined_at(frame, &d.name))
        .collect()
}

/// Structures that run concurrently because of the spawn `call` of `node`.
fn lanes(node: &Node, call: &CallSite, kind: SpawnKind) -> Vec<NodeId> {
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
    lanes
}

/// Thread name from `Builder::name("...")`.
fn thread_name(call: &CallSite) -> Option<String> {
    call.literals
        .iter()
        .find(|(link, _)| link == "name")
        .map(|(_, v)| v.clone())
}

/// Functions called in the lanes of a spawn (each once, in source order),
/// without the calls that say nothing about what the lane does.
fn lane_calls(body: &Body, node: &Node, spawn: &CallSite, lanes: &[NodeId]) -> Vec<String> {
    let mut runs = Vec::new();
    for &lane in lanes {
        collect_calls(body, lane, &mut runs);
    }
    if lanes.is_empty() {
        // Inline closure: its calls were recorded on this node.
        runs.extend(
            node.calls
                .iter()
                .filter(|c| c.kind == CallKind::Function && !std::ptr::eq(*c, spawn))
                .map(|c| c.path.join("::")),
        );
    }
    runs.retain(|r| !is_plumbing(r));
    let mut seen = HashSet::new();
    runs.retain(|r| seen.insert(r.clone()));
    runs
}

/// Recognize a call that starts concurrent work.
fn classify_spawn(
    call: &CallSite,
    scope_kind: impl Fn(&str) -> Option<SpawnKind>,
) -> Option<(SpawnKind, String)> {
    match call.kind {
        CallKind::Function => spawn_function(call),
        CallKind::Method => spawn_method(call, scope_kind),
        CallKind::Macro => {
            let kind = match call.name() {
                "join" | "try_join" => SpawnKind::Join,
                "select" | "select_biased" => SpawnKind::Select,
                _ => return None,
            };
            Some((kind, format!("{}!", call.path.join("::"))))
        }
    }
}

/// A free function that starts concurrent work (`thread::spawn`, `tokio::spawn`, ...).
fn spawn_function(call: &CallSite) -> Option<(SpawnKind, String)> {
    let name = call.name();
    let has = |seg: &str| call.path.iter().any(|p| p == seg);
    let kind = if call.ends_with(&["thread", "spawn"]) {
        return Some((SpawnKind::Thread, "thread::spawn".into()));
    } else if name == "spawn_blocking" {
        SpawnKind::Blocking
    } else if let Some(kind) = combinator(call) {
        kind
    } else if (name == "spawn" || name == "spawn_local") && ASYNC_RUNTIMES.iter().any(|r| has(r)) {
        SpawnKind::AsyncTask
    } else if name == "block_on" {
        SpawnKind::BlockOn
    } else {
        return None;
    };
    Some((kind, call.path.join("::")))
}

/// The kind of a call to one of the [`COMBINATORS`].
fn combinator(call: &CallSite) -> Option<SpawnKind> {
    COMBINATORS
        .iter()
        .find(|(path, _)| call.ends_with(path))
        .map(|&(_, kind)| kind)
}

/// A method that starts concurrent work (`scope.spawn`, `runtime.block_on`,
/// `items.par_iter()`, ...).
fn spawn_method(
    call: &CallSite,
    scope_kind: impl Fn(&str) -> Option<SpawnKind>,
) -> Option<(SpawnKind, String)> {
    let name = call.name();
    let has = |seg: &str| call.path.iter().any(|p| p == seg);
    let on_receiver = |default: &str, method: &str| {
        format!("{}.{method}", call.receiver.as_deref().unwrap_or(default))
    };
    if name == "spawn" || name == "spawn_scoped" {
        if call.args.is_empty() || has("Command") {
            // `std::process::Command::spawn()` starts a process, not a task.
            None
        } else if has("Builder") {
            Some((SpawnKind::Thread, "thread::Builder::spawn".into()))
        } else if let Some(kind) = call.receiver.as_deref().and_then(&scope_kind) {
            Some((kind, on_receiver("scope", "spawn")))
        } else {
            Some((SpawnKind::AsyncTask, on_receiver("executor", "spawn")))
        }
    } else if name == "spawn_blocking" {
        Some((SpawnKind::Blocking, call.path.join("::")))
    } else if name == "block_on" {
        Some((SpawnKind::BlockOn, on_receiver("runtime", "block_on")))
    } else if PAR_ITER_METHODS.contains(&name) {
        Some((SpawnKind::ParallelIter, name.to_string()))
    } else {
        None
    }
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
