//! Concurrency facts of one function: what runs in parallel (lanes), which
//! channels connect them and which shared state they touch.
//!
//! Recognition is by API name (syntactic): the tables in the submodules list
//! the paths and methods that are understood.

use std::collections::HashMap;

use rv_ir::{Body, NodeId};

use crate::DataFlow;

mod channels;
mod shared;
mod spawns;
mod uses;

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
    let mut c = Concurrency {
        spawns: spawns::spawns(body, flow),
        channels: channels::channels(body, flow),
        shared: shared::shared_groups(body, flow, aliases),
        ..Concurrency::default()
    };
    uses::alias_clones(body, flow, &mut c);
    let (channel_uses, shared_uses) = uses::uses(body, flow, &c);
    c.channel_uses = channel_uses;
    c.shared_uses = shared_uses;
    c
}
