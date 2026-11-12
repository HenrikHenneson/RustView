//! What the user changed in a diagram (expansion, shown cases, expression
//! level) and the defaults everything else follows.

use std::collections::{HashMap, HashSet, VecDeque};

use rv_ir::{Body, FrameRole, NodeId, NodeKind};

/// Case structures with more frames show one case at a time by default.
pub const MAX_SIDE_BY_SIDE: usize = 4;

/// What the user changed; everything else follows the defaults.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub expanded: HashMap<NodeId, bool>,
    /// Shown case of case structures in one-at-a-time mode.
    pub case_index: HashMap<NodeId, usize>,
    pub all_cases: HashMap<NodeId, bool>,
    /// Structures whose statements show their expressions as data flow.
    pub expressions: HashSet<NodeId>,
}

impl ViewState {
    pub fn is_expanded(&self, defaults: &HashSet<NodeId>, id: NodeId) -> bool {
        self.expanded
            .get(&id)
            .copied()
            .unwrap_or_else(|| defaults.contains(&id))
    }

    pub fn shows_all_cases(&self, body: &Body, id: NodeId) -> bool {
        self.all_cases
            .get(&id)
            .copied()
            .unwrap_or_else(|| case_frames(body, id).len() <= MAX_SIDE_BY_SIDE)
    }

    pub fn case_index(&self, body: &Body, id: NodeId) -> usize {
        let count = case_frames(body, id).len();
        self.case_index
            .get(&id)
            .copied()
            .unwrap_or(0)
            .min(count.saturating_sub(1))
    }

    /// Frames of a structure that are currently shown.
    pub fn visible_frames(&self, body: &Body, id: NodeId) -> Vec<NodeId> {
        let node = body.node(id);
        let cases = case_frames(body, id);
        if cases.len() < 2 || self.shows_all_cases(body, id) {
            return node.children.clone();
        }
        let shown = cases[self.case_index(body, id)];
        node.children
            .iter()
            .copied()
            .filter(|&f| f == shown || !cases.contains(&f))
            .collect()
    }

    /// Opens structure `s`: queues what becomes visible inside it and
    /// returns the number of items that adds.
    fn open(&self, body: &Body, s: NodeId, queue: &mut VecDeque<NodeId>) -> usize {
        let mut cost = 0;
        for f in self.visible_frames(body, s) {
            for &item in &body.node(f).children {
                cost += 1;
                match body.node(item).kind {
                    NodeKind::Structure { .. } => queue.push_back(item),
                    _ => queue.extend(body.node(item).children.iter().copied()),
                }
            }
        }
        cost
    }
}

/// `Case` and `Else` frames (the alternatives) of a structure.
pub fn case_frames(body: &Body, id: NodeId) -> Vec<NodeId> {
    body.node(id)
        .children
        .iter()
        .copied()
        .filter(|&f| {
            matches!(
                body.node(f).kind,
                NodeKind::Frame {
                    role: FrameRole::Case { .. } | FrameRole::Else
                }
            )
        })
        .collect()
}

/// Structures to expand so that about `budget` items are visible,
/// breadth first from the outside in. The root is always expanded.
pub fn default_expanded(body: &Body, state: &ViewState, budget: usize) -> HashSet<NodeId> {
    let mut expanded = HashSet::from([body.root]);
    let mut queue = VecDeque::new();
    let mut visible = state.open(body, body.root, &mut queue);
    while let Some(s) = queue.pop_front() {
        let mut probe = VecDeque::new();
        let cost = state.open(body, s, &mut probe);
        if visible + cost <= budget {
            visible += cost;
            expanded.insert(s);
            queue.extend(probe);
        }
    }
    expanded
}
