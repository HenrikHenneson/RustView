//! How the threads found by the walk communicate: channels, shared state
//! and join handles, each followed from where it is created to its uses.

use std::collections::BTreeSet;

use rv_ir::{Body, CallSite, FnId, FnOwner, NodeId};

use super::model::{ChannelComm, JoinComm, Link, LinkKind, Program, Resource, SharedComm, Site};
use super::resolver::self_type;
use super::walker::Walker;
use crate::Concurrency;
use crate::concurrency::Channel;

/// Adds the channels, shared state and joins of all reached functions to
/// `program`.
pub(super) fn connect(w: &Walker, program: &mut Program) {
    for (i, f) in w.project.functions.iter().enumerate() {
        let id = FnId(i as u32);
        let (Some(conc), Some(body)) = (w.facts.concurrency(id), f.body.as_ref()) else {
            continue;
        };
        let origin = Origin {
            w,
            f: id,
            body,
            conc,
        };
        origin.channels(program);
        origin.shared_created(program);
        origin.shared_returned(program);
        origin.joins(program);
    }
}

/// A function in which communication objects are created.
struct Origin<'a> {
    w: &'a Walker<'a>,
    f: FnId,
    body: &'a Body,
    conc: &'a Concurrency,
}

impl Origin<'_> {
    /// No thread executes `node`: the function is not reached from this entry.
    fn unreached(&self, node: NodeId) -> bool {
        self.w.threads_at(self.f, node).is_empty()
    }

    /// Channels created here, with a link for every pair of sending and
    /// receiving thread.
    fn channels(&self, program: &mut Program) {
        for ch in &self.conc.channels {
            if self.unreached(ch.def) {
                continue;
            }
            let comm = self.channel(ch);
            let s_threads: BTreeSet<usize> = comm.senders.iter().map(|s| s.thread).collect();
            let r_threads: BTreeSet<usize> = comm.receivers.iter().map(|s| s.thread).collect();
            for &s in &s_threads {
                for &r in r_threads.iter().filter(|&&r| r != s) {
                    program.links.push(Link {
                        from: s,
                        to: r,
                        kind: LinkKind::Channel {
                            name: ch.name.clone(),
                            api: ch.api.clone(),
                        },
                        site: (self.f, ch.def),
                    });
                }
            }
            program.channels.push(comm);
        }
    }

    /// Who sends and receives on `ch`, and what is known about it.
    fn channel(&self, ch: &Channel) -> ChannelComm {
        let sent = self.w.trail(self.f, &ch.senders);
        let received = self.w.trail(self.f, &ch.receivers);
        let (mut senders, mut receivers) = (sent.sites, received.sites);
        for list in [&mut senders, &mut receivers] {
            if list.is_empty() {
                self.w.sites(self.f, ch.def, None, list);
            }
        }
        let ctor = self
            .body
            .node(ch.def)
            .calls
            .iter()
            .find(|c| ch.api == c.path.join("::"));
        let mut types = sent.types.iter().chain(&received.types);
        let message = ctor
            .and_then(|c| c.turbofish.clone())
            .or_else(|| types.find_map(|t| generic_arg(t)));
        ChannelComm {
            name: ch.name.clone(),
            api: ch.api.clone(),
            message,
            capacity: ctor.and_then(capacity),
            site: (self.f, ch.def),
            senders,
            receivers,
        }
    }

    /// Shared state created here (`Arc::new`).
    fn shared_created(&self, program: &mut Program) {
        for g in &self.conc.shared {
            let created = self
                .body
                .node(g.def)
                .calls
                .iter()
                .any(|c| c.ends_with(&["Arc", "new"]));
            if !created || self.unreached(g.def) {
                continue;
            }
            self.share(&g.name, &g.kind, g.def, &g.bindings, program);
        }
    }

    /// Shared state from constructors returning `Arc<…>` (`Geteilt::neu()`).
    fn shared_returned(&self, program: &mut Program) {
        let Some(flow) = self.w.facts.flow(self.f) else {
            return;
        };
        for (n, node) in self.body.nodes.iter().enumerate() {
            let nid = NodeId(n as u32);
            if node.defs.len() != 1
                || self.conc.shared.iter().any(|g| g.def == nid)
                || self.unreached(nid)
            {
                continue;
            }
            let Some(kind) = self.shared_call_kind(nid) else {
                continue;
            };
            let name = &node.defs[0].name;
            if let Some(b) = flow.defined_at(nid, name) {
                self.share(name, &kind, nid, &[b], program);
            }
        }
    }

    /// Kind of the shared object a call in `node` returns, if one of them
    /// is a project function returning `Arc<…>`.
    fn shared_call_kind(&self, node: NodeId) -> Option<String> {
        let w = self.w;
        (0..self.body.node(node).calls.len()).find_map(|ci| {
            let callee = w.project.function(w.graph.callee(self.f, node, ci)?);
            shared_return_kind(callee.sig.ret.as_deref()?, &callee.owner)
        })
    }

    /// Records the shared object held by `bindings` (defined in `node`) if
    /// more than one thread uses it.
    fn share(
        &self,
        name: &str,
        kind: &str,
        node: NodeId,
        bindings: &[usize],
        program: &mut Program,
    ) {
        let shared = SharedComm {
            name: name.to_string(),
            kind: kind.to_string(),
            site: (self.f, node),
            users: self.w.trail(self.f, bindings).sites,
        };
        let threads = shared.threads();
        if threads.len() >= 2 {
            program.resources.push(Resource {
                name: shared.name.clone(),
                kind: shared.kind.clone(),
                site: shared.site,
                users: threads.into_iter().collect(),
            });
            program.shared.push(shared);
        }
    }

    /// Join handles of threads started here.
    fn joins(&self, program: &mut Program) {
        let Some(flow) = self.w.facts.flow(self.f) else {
            return;
        };
        for (si, s) in self.conc.spawns.iter().enumerate() {
            let Some(&thread) = self.w.spawn_thread.get(&(self.f, si)) else {
                continue;
            };
            let node = self.body.node(s.node);
            if node.defs.len() != 1 {
                continue;
            }
            let Some(handle) = flow.defined_at(s.node, &node.defs[0].name) else {
                continue;
            };
            let waiters: Vec<Site> = self
                .w
                .trail(self.f, &[handle])
                .sites
                .into_iter()
                .filter(|s| s.op.as_deref() == Some("join"))
                .collect();
            if !waiters.is_empty() {
                program.joins.push(JoinComm { thread, waiters });
            }
        }
    }
}

/// Capacity argument of a bounded channel constructor, as written.
fn capacity(ctor: &CallSite) -> Option<String> {
    (matches!(ctor.name(), "sync_channel" | "bounded" | "channel") && !ctor.arg_texts.is_empty())
        .then(|| ctor.arg_texts[0].clone())
}

/// `Arc<Mutex<T>>` -> "Mutex", `Arc<Self>` in `impl Geteilt` -> "Arc<Geteilt>".
fn shared_return_kind(ret: &str, owner: &FnOwner) -> Option<String> {
    let start = ret.find("Arc<")? + 4;
    let inner = ret.get(start..ret.rfind('>')?)?.trim();
    for kind in ["Mutex", "RwLock", "Condvar"] {
        if inner.starts_with(kind) {
            return Some(kind.into());
        }
    }
    if inner.starts_with("Atomic") {
        return Some("Atomic".into());
    }
    let inner = if inner == "Self" {
        self_type(owner)?
    } else {
        inner
    };
    Some(format!("Arc<{inner}>"))
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
