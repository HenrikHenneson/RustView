//! What the diagram derives from the analyses of one function: the wires to
//! draw, the notes next to labels, the colors of parallel lanes and the
//! shift registers of loops. Computed once per function and project
//! generation.

use std::collections::HashMap;

use egui::Color32;
use rv_analysis::concurrency::Spawn;
use rv_analysis::{Concurrency, DataFlow, EdgeKind, End, SpawnKind};
use rv_ir::{Body, NodeId, Project};

use super::palette::LANE_COLORS;
use crate::wires::{WireKind, WireSpec};

/// Analyses of one function and what the diagram derives from them.
pub(super) struct Analysis {
    pub(super) flow: DataFlow,
    pub(super) conc: Concurrency,
    pub(super) specs: Vec<WireSpec>,
    pub(super) notes: rv_layout::Notes,
    /// Lane structures and statements that start an inline lane.
    pub(super) lane_color: HashMap<NodeId, Color32>,
    /// Shift registers per loop.
    pub(super) registers: HashMap<NodeId, Vec<String>>,
}

impl Analysis {
    pub(super) fn new(body: &Body, project: &Project) -> Self {
        let flow = rv_analysis::data_flow(body);
        let conc = rv_analysis::concurrency(body, &flow, &project.type_aliases);
        let mut specs = data_specs(&flow);
        specs.extend(channel_specs(&conc));
        specs.extend(shared_specs(body, &conc));
        let (mut notes, lane_color) = lanes(&conc);
        let registers = shift_registers(&flow);
        // Shift registers: one note per loop.
        for (lp, names) in &registers {
            add_note(&mut notes, *lp, format!("⟳ {}", names.join(", ")));
        }
        Analysis {
            registers,
            flow,
            conc,
            specs,
            notes,
            lane_color,
        }
    }
}

/// One wire per data flow edge.
fn data_specs(flow: &DataFlow) -> Vec<WireSpec> {
    flow.edges
        .iter()
        .enumerate()
        .map(|(i, e)| WireSpec {
            from: e.from,
            to: e.to,
            kind: match e.kind {
                EdgeKind::Data => WireKind::Data,
                EdgeKind::Mutation => WireKind::Mutation,
                EdgeKind::PossibleMutation => WireKind::PossibleMutation,
                EdgeKind::LoopCarried => WireKind::LoopBack,
            },
            certain: e.certain,
            name: flow.binding(e).name.clone(),
            source: i,
        })
        .collect()
}

/// Channels: every send-side place to every receive-side place.
fn channel_specs(conc: &Concurrency) -> Vec<WireSpec> {
    let mut specs = Vec::new();
    for (ci, ch) in conc.channels.iter().enumerate() {
        let ends = |end: End| {
            conc.channel_uses
                .iter()
                .filter(move |u| u.channel == ci && u.end == end)
        };
        for s in ends(End::Send) {
            for r in ends(End::Recv).filter(|r| r.node != s.node) {
                specs.push(WireSpec {
                    from: s.node,
                    to: r.node,
                    kind: WireKind::Channel,
                    certain: true,
                    name: ch.name.clone(),
                    source: ci,
                });
            }
        }
    }
    specs
}

/// Shared state: from where it is created to uses in other lanes and to the
/// statements that hand it to a new lane.
fn shared_specs(body: &Body, conc: &Concurrency) -> Vec<WireSpec> {
    let spawn_nodes: Vec<NodeId> = conc.spawns.iter().map(|s| s.node).collect();
    let mut specs = Vec::new();
    for (gi, g) in conc.shared.iter().enumerate() {
        let home = conc.lane_of(body, g.def);
        let leaves_home =
            |node: NodeId| conc.lane_of(body, node) != home || spawn_nodes.contains(&node);
        for u in conc.shared_uses.iter().filter(|u| u.group == gi) {
            if leaves_home(u.node) {
                specs.push(WireSpec {
                    from: g.def,
                    to: u.node,
                    kind: WireKind::Shared,
                    certain: true,
                    name: format!("{} ({})", g.name, g.kind),
                    source: gi,
                });
            }
        }
    }
    specs
}

/// Lane badges (as notes) and lane colors of everything that is spawned.
fn lanes(conc: &Concurrency) -> (rv_layout::Notes, HashMap<NodeId, Color32>) {
    let mut notes: rv_layout::Notes = HashMap::new();
    let mut lane_color = HashMap::new();
    let mut next_color = 0;
    for sp in &conc.spawns {
        let label = spawn_label(sp);
        let color = sp.kind.is_parallel().then(|| {
            let c = LANE_COLORS[next_color % LANE_COLORS.len()];
            next_color += 1;
            c
        });
        // Without lane structures the spawning statement stands for the lane.
        let targets: &[NodeId] = if sp.lanes.is_empty() {
            std::slice::from_ref(&sp.node)
        } else {
            &sp.lanes
        };
        for &t in targets {
            add_note(&mut notes, t, label.clone());
            if let Some(c) = color {
                lane_color.entry(t).or_insert(c);
            }
        }
    }
    (notes, lane_color)
}

/// Badge of a spawn: kind, name, `×n` in loops and, without lane
/// structures, what runs in it.
fn spawn_label(sp: &Spawn) -> String {
    let mut label = match (&sp.name, sp.kind) {
        (Some(name), _) => format!("‖ {} '{name}'", sp.kind.label()),
        (None, SpawnKind::BlockOn) => "async on this thread (block_on)".to_string(),
        (None, kind) => format!("‖ {}", kind.label()),
    };
    if sp.in_loop {
        label.push_str(" ×n");
    }
    if sp.lanes.is_empty()
        && let Some(first) = sp.runs.first()
    {
        label.push_str(&format!(" → {first}"));
    }
    label
}

/// Names of the values carried into the next iteration, per loop.
fn shift_registers(flow: &DataFlow) -> HashMap<NodeId, Vec<String>> {
    let mut registers: HashMap<NodeId, Vec<String>> = HashMap::new();
    for &(lp, b) in &flow.shift_registers {
        registers
            .entry(lp)
            .or_default()
            .push(flow.bindings[b].name.clone());
    }
    registers
}

/// Adds `label` to the notes of `node`, after what is already there.
fn add_note(notes: &mut rv_layout::Notes, node: NodeId, label: String) {
    notes
        .entry(node)
        .and_modify(|n| {
            n.push_str(" · ");
            n.push_str(&label);
        })
        .or_insert(label);
}
