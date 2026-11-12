//! Channels created in a body and the names of the operations on their ends.

use rv_ir::{Body, CallKind, CallSite, NodeId};

use super::{Channel, End};
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

/// Channels created by `let (tx, rx) = channel()`, in source order.
pub(super) fn channels(body: &Body, flow: &DataFlow) -> Vec<Channel> {
    let mut channels = Vec::new();
    for (n, node) in body.nodes.iter().enumerate() {
        let id = NodeId(n as u32);
        if node.defs.len() != 2 {
            continue;
        }
        let Some(ctor) = node.calls.iter().find(|c| is_channel_ctor(c)) else {
            continue;
        };
        let (Some(tx), Some(rx)) = (
            flow.defined_at(id, &node.defs[0].name),
            flow.defined_at(id, &node.defs[1].name),
        ) else {
            continue;
        };
        channels.push(Channel {
            def: id,
            api: ctor.path.join("::"),
            name: format!("{} → {}", node.defs[0].name, node.defs[1].name),
            senders: vec![tx],
            receivers: vec![rx],
        });
    }
    channels
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

/// The end of `channel` that `binding` holds, if it is one of its ends.
pub(super) fn end_of(channel: &Channel, binding: usize) -> Option<End> {
    if channel.senders.contains(&binding) {
        Some(End::Send)
    } else if channel.receivers.contains(&binding) {
        Some(End::Recv)
    } else {
        None
    }
}

/// How a method called on a channel end is recorded: its name if it is a
/// known operation of that end, otherwise marked as some other call (`len()`).
pub(super) fn op_name(end: End, method: &str) -> String {
    let known = match end {
        End::Send => SEND_METHODS.contains(&method),
        End::Recv => RECV_METHODS.contains(&method),
    };
    if known {
        method.to_string()
    } else {
        format!("{method}()")
    }
}
