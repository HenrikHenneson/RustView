//! The result of the whole-program walk: threads, what connects them and
//! what the operations on channels and shared state mean.

use std::collections::BTreeSet;

use rv_ir::{FnId, NodeId};

use crate::SpawnKind;

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

/// Whether an operation on shared state (as in [`Site::op`]) may change it.
pub fn op_writes(op: &str) -> bool {
    let method = op.rsplit('.').next().unwrap_or(op);
    !matches!(method, "load" | "read" | "try_read" | "is_poisoned") && !op.starts_with("stored in")
}

/// Short effect of an operation (as in [`Site::op`]) on the calling thread:
/// `blocking`, `non-blocking`, ... `None` where nothing can be said (a method
/// unknown to the analysis, a value kept in a struct field).
pub fn op_effect(op: Option<&str>, bounded: bool) -> Option<&'static str> {
    let Some(op) = op else {
        return Some("not followed");
    };
    if op.starts_with("stored in ") {
        return None;
    }
    let op = op.rsplit('.').next().unwrap_or(op);
    Some(match op {
        "send" | "blocking_send" | "send_blocking" if bounded => "blocking",
        "send" => "non-blocking",
        "try_send" => "non-blocking, may drop",
        "send_timeout" | "recv_timeout" | "recv_deadline" | "wait_timeout" => {
            "blocking with timeout"
        }
        "recv" | "blocking_recv" | "recv_async" | "iter" | "lock" | "write" | "read" | "join"
        | "wait" | "wait_while" => "blocking",
        "try_recv" | "try_iter" | "try_lock" | "try_write" | "try_read" | "notify_one"
        | "notify_all" => "non-blocking",
        "load"
        | "store"
        | "swap"
        | "compare_exchange"
        | "compare_exchange_weak"
        | "fetch_update" => "atomic",
        op if op.starts_with("fetch_") => "atomic",
        _ => return None,
    })
}

/// What happens at an operation (as in [`Site::op`]), in words.
pub fn op_meaning(op: Option<&str>, bounded: bool) -> String {
    let Some(op) = op else {
        return "handed on or captured (not followed further)".into();
    };
    if let Some(field) = op.strip_prefix("stored in ") {
        return format!("kept in the field {field}; its methods use it");
    }
    // `stand.lock`: the operation is on a field of the shared object.
    let op = op.rsplit('.').next().unwrap_or(op);
    let s = match op {
        "send" | "blocking_send" | "send_blocking" if bounded => {
            "sends; blocks while the channel is full"
        }
        "send" => "sends; does not block (unbounded)",
        "try_send" => "sends if there is room; fails when full (message lost unless handled)",
        "send_timeout" => "sends; waits at most a timeout when full",
        "recv" | "blocking_recv" | "recv_async" => "waits until a message arrives",
        "try_recv" => "polls; returns at once if nothing is there",
        "recv_timeout" | "recv_deadline" => "waits for a message, at most a timeout",
        "iter" => "loops over messages, waiting for each",
        "try_iter" => "takes what is there without waiting",
        "lock" | "write" => "exclusive lock; waits while another thread holds it",
        "try_lock" | "try_write" => "tries to lock; does not wait",
        "read" => "shared read lock; waits while a writer holds it",
        "load" => "atomic read",
        "store" => "atomic write",
        "swap" | "compare_exchange" | "compare_exchange_weak" | "fetch_update" => {
            "atomic read-modify-write"
        }
        "join" => "waits until the thread has finished",
        "notify_one" | "notify_all" => "wakes waiting threads",
        "wait" | "wait_timeout" | "wait_while" => "sleeps until notified",
        op if op.starts_with("fetch_") => "atomic read-modify-write",
        _ => "method on the object (what it does inside is not analyzed)",
    };
    s.to_string()
}
