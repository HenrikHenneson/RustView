//! Shared state of a body: values created with `Arc::new` and bindings
//! whose type says they are shared (`Arc<Mutex<T>>`, `&AtomicBool`).

use std::collections::HashMap;

use rv_ir::{Body, CallSite, NodeId};

use super::SharedGroup;
use crate::DataFlow;

/// One group per shared binding, in source order.
pub(super) fn shared_groups(
    body: &Body,
    flow: &DataFlow,
    aliases: &HashMap<String, String>,
) -> Vec<SharedGroup> {
    let mut groups = Vec::new();
    for (n, node) in body.nodes.iter().enumerate() {
        let id = NodeId(n as u32);
        let created = node.calls.iter().any(|c| c.ends_with(&["Arc", "new"]));
        for d in &node.defs {
            let Some(b) = flow.defined_at(id, &d.name) else {
                continue;
            };
            let kind = if created && node.defs.len() == 1 {
                Some(shared_kind_of_calls(&node.calls))
            } else {
                d.ty.as_deref()
                    .and_then(|t| shared_kind_of_type(t, aliases))
            };
            if let Some(kind) = kind {
                groups.push(SharedGroup {
                    def: id,
                    name: d.name.clone(),
                    kind,
                    bindings: vec![b],
                });
            }
        }
    }
    groups
}

fn shared_kind_of_calls(calls: &[CallSite]) -> String {
    for seg in calls.iter().flat_map(|c| &c.path) {
        for kind in ["Mutex", "RwLock", "Condvar"] {
            if seg == kind {
                return kind.into();
            }
        }
        if seg.starts_with("Atomic") {
            return "Atomic".into();
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
