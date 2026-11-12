//! Where channel ends and shared handles are used: clones become aliases,
//! method calls are operations, every other use hands the value on.

use std::collections::HashSet;

use rv_ir::{Body, CallKind, NodeId};

use super::channels::{end_of, op_name};
use super::{ChannelUse, Concurrency, End, SharedUse};
use crate::DataFlow;

/// Clones of channel ends and of shared handles are aliases: adds the new
/// binding to the channel end / shared group of its source.
pub(super) fn alias_clones(body: &Body, flow: &DataFlow, c: &mut Concurrency) {
    for n in 0..body.nodes.len() {
        let Some((src, new)) = clone_source(body, flow, NodeId(n as u32)) else {
            continue;
        };
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

/// `(source, new)` bindings of `let new = source.clone()` /
/// `let new = Arc::clone(&source)`.
fn clone_source(body: &Body, flow: &DataFlow, id: NodeId) -> Option<(usize, usize)> {
    let node = body.node(id);
    if node.defs.len() != 1 {
        return None;
    }
    let new = flow.defined_at(id, &node.defs[0].name)?;
    let src_name = node.calls.iter().find_map(|c| {
        if c.kind == CallKind::Method && c.path.first().map(String::as_str) == Some("clone") {
            c.receiver.clone()
        } else if c.kind == CallKind::Function && c.name() == "clone" {
            c.args.first().cloned().flatten()
        } else {
            None
        }
    })?;
    Some((flow.binding_at(id, &src_name)?, new))
}

/// The uses of the channels and shared groups of `c`, in source order; at
/// most one per node and channel end / shared group.
pub(super) fn uses(
    body: &Body,
    flow: &DataFlow,
    c: &Concurrency,
) -> (Vec<ChannelUse>, Vec<SharedUse>) {
    let mut uses = Uses {
        body,
        flow,
        c,
        channel_uses: Vec::new(),
        shared_uses: Vec::new(),
        seen_channels: HashSet::new(),
        seen_shared: HashSet::new(),
    };
    for n in 0..body.nodes.len() {
        uses.node(NodeId(n as u32));
    }
    (uses.channel_uses, uses.shared_uses)
}

/// Collects the uses, node by node.
struct Uses<'a> {
    body: &'a Body,
    flow: &'a DataFlow,
    /// Its channels and shared groups are looked up.
    c: &'a Concurrency,
    channel_uses: Vec<ChannelUse>,
    shared_uses: Vec<SharedUse>,
    /// (node, channel, end) already recorded.
    seen_channels: HashSet<(NodeId, usize, End)>,
    /// (node, group) already recorded.
    seen_shared: HashSet<(NodeId, usize)>,
}

impl Uses<'_> {
    fn node(&mut self, id: NodeId) {
        let node = self.body.node(id);
        let mut receivers = HashSet::new();
        for call in &node.calls {
            let Some(r) = &call.receiver else { continue };
            let Some(b) = self.flow.binding_at(id, r) else {
                continue;
            };
            receivers.insert(r.clone());
            let method = call.path.first().cloned().unwrap_or_default();
            if method != "clone" {
                self.method_call(id, b, &method);
            }
        }
        // Passed on: used, but not as the receiver of a call.
        for u in &node.uses {
            if receivers.contains(&u.name) {
                continue;
            }
            if let Some(b) = self.flow.binding_at(id, &u.name) {
                self.passed_on(id, b);
            }
        }
    }

    /// `method` is called on `binding` in node `id`.
    fn method_call(&mut self, id: NodeId, binding: usize, method: &str) {
        for (ci, ch) in self.c.channels.iter().enumerate() {
            if let Some(end) = end_of(ch, binding) {
                self.channel_use(id, ci, end, Some(op_name(end, method)));
            }
        }
        for (gi, g) in self.c.shared.iter().enumerate() {
            if g.bindings.contains(&binding) && self.seen_shared.insert((id, gi)) {
                self.shared_uses.push(SharedUse {
                    node: id,
                    group: gi,
                    op: Some(method.to_string()),
                });
            }
        }
    }

    /// `binding` is used in node `id` without a method being called on it
    /// (moved into a call, captured, returned).
    fn passed_on(&mut self, id: NodeId, binding: usize) {
        for (ci, ch) in self.c.channels.iter().enumerate() {
            if ch.def == id {
                continue;
            }
            if let Some(end) = end_of(ch, binding) {
                self.channel_use(id, ci, end, None);
            }
        }
        for (gi, g) in self.c.shared.iter().enumerate() {
            if g.bindings.contains(&binding)
                && clone_source(self.body, self.flow, id).is_none_or(|(src, _)| src != binding)
                && self.seen_shared.insert((id, gi))
            {
                self.shared_uses.push(SharedUse {
                    node: id,
                    group: gi,
                    op: None,
                });
            }
        }
    }

    /// Records the first use of a channel end in a node.
    fn channel_use(&mut self, id: NodeId, channel: usize, end: End, op: Option<String>) {
        if self.seen_channels.insert((id, channel, end)) {
            self.channel_uses.push(ChannelUse {
                node: id,
                channel,
                end,
                op,
            });
        }
    }
}
