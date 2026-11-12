//! Threads and tasks by the function that starts them: per spawning
//! function its lanes with the channels and shared state each one touches.

use rv_analysis::{Concurrency, End};
use rv_ir::{Body, FnId, NodeId, Project};

pub(super) struct Overview {
    pub groups: Vec<Group>,
    pub channels: usize,
    pub shared: usize,
    pub lanes: usize,
}

/// A function that starts lanes.
pub(super) struct Group {
    pub func: FnId,
    /// The function itself (the spawning thread) and its lanes.
    pub cards: Vec<Card>,
}

pub(super) struct Card {
    pub title: String,
    pub node: Option<NodeId>,
    pub line: u32,
    pub parallel: bool,
    pub runs: Vec<(String, Option<FnId>)>,
    /// (channel name, color index, end, operation)
    pub channels: Vec<(String, usize, End, Option<String>)>,
    pub shared: Vec<String>,
}

pub(super) fn overview(project: &Project) -> Overview {
    let mut o = Overview {
        groups: Vec::new(),
        channels: 0,
        shared: 0,
        lanes: 0,
    };
    for (i, func) in project.functions.iter().enumerate() {
        let Some(body) = &func.body else { continue };
        let flow = rv_analysis::data_flow(body);
        let c = rv_analysis::concurrency(body, &flow, &project.type_aliases);
        if !c.spawns.iter().any(|s| s.kind.is_parallel()) {
            continue;
        }
        let f = FnId(i as u32);
        let crate_name = func.qual_path.split("::").next().unwrap_or("");
        let mut cards = vec![Card {
            title: format!("{} (this thread)", func.name),
            node: None,
            line: func.name_span.start.line,
            parallel: false,
            runs: Vec::new(),
            channels: lane_channels(body, &c, None),
            shared: lane_shared(body, &c, None),
        }];
        for (si, s) in c
            .spawns
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind.is_parallel())
        {
            o.lanes += 1;
            let mut title = s.kind.label().to_string();
            if let Some(name) = &s.name {
                title.push_str(&format!(" '{name}'"));
            }
            if s.in_loop {
                title.push_str(" ×n");
            }
            cards.push(Card {
                title,
                node: Some(s.node),
                line: body.node(s.node).span.start.line,
                parallel: true,
                runs: s
                    .runs
                    .iter()
                    .map(|r| (r.clone(), resolve(project, crate_name, r)))
                    .collect(),
                channels: lane_channels(body, &c, Some(si)),
                shared: lane_shared(body, &c, Some(si)),
            });
        }
        o.channels += c.channels.len();
        o.shared += c.shared.len();
        o.groups.push(Group { func: f, cards });
    }
    o
}

/// Lane of a node for the overview: the spawn statement itself belongs to
/// its (inline) lane if it has no structure lanes.
fn lane(body: &Body, c: &Concurrency, node: NodeId) -> Option<usize> {
    c.lane_of(body, node).or_else(|| {
        c.spawns
            .iter()
            .position(|s| s.node == node && s.lanes.is_empty() && s.kind.is_parallel())
    })
}

fn lane_channels(
    body: &Body,
    c: &Concurrency,
    l: Option<usize>,
) -> Vec<(String, usize, End, Option<String>)> {
    let mut out: Vec<(String, usize, End, Option<String>)> = Vec::new();
    for u in &c.channel_uses {
        if lane(body, c, u.node) != l {
            continue;
        }
        let entry = (
            c.channels[u.channel].name.clone(),
            u.channel,
            u.end,
            u.op.clone(),
        );
        if !out.iter().any(|e| e.1 == entry.1 && e.2 == entry.2) {
            out.push(entry);
        }
    }
    out
}

fn lane_shared(body: &Body, c: &Concurrency, l: Option<usize>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in &c.shared_uses {
        if lane(body, c, u.node) != l {
            continue;
        }
        let g = &c.shared[u.group];
        let name = format!("{} ({})", g.name, g.kind);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Project function a called path refers to, preferring the caller's crate.
fn resolve(project: &Project, crate_name: &str, path: &str) -> Option<FnId> {
    let path = path
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .trim_start_matches("super::");
    let suffix = format!("::{path}");
    let candidates: Vec<usize> = project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path.ends_with(&suffix))
        .map(|(i, _)| i)
        .collect();
    candidates
        .iter()
        .find(|&&i| project.functions[i].qual_path.starts_with(crate_name))
        .or(candidates.first())
        .map(|&i| FnId(i as u32))
}
