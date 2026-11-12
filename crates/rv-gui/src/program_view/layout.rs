//! Geometry of the program graph: thread boxes in columns by spawn depth,
//! shared state in a column of its own, and the arrows between them.

use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Vec2, pos2, vec2};
use rv_analysis::{LinkKind, Program, Thread};
use rv_ir::{FnId, NodeId, Project};

use crate::canvas::bezier;
use crate::lookup::node_line;
use crate::style::{CHANNEL, SHARED, SPAWN};

const BOX_W: f32 = 280.0;
const COL_GAP: f32 = 140.0;
const ROW_GAP: f32 = 28.0;
/// Height of a text line in a box.
pub(super) const LINE: f32 = 17.0;
/// Space between the border of a box and its text.
pub(super) const PAD: f32 = 8.0;
const MARGIN: f32 = 20.0;
/// Longer lines in a box are cut off with "…".
const MAX_LINE_CHARS: usize = 40;
/// Functions named per thread box (entries and callbacks each).
const MAX_NAMES: usize = 3;
/// Distance between several arrows connecting the same two threads.
const FAN_OUT: f32 = 14.0;
/// Segments an arrow curve is drawn with.
const CURVE_STEPS: usize = 24;

/// Geometry of one program graph.
pub struct GraphLayout {
    pub(super) boxes: Vec<Block>,
    pub(super) arrows: Vec<Arrow>,
    pub(super) size: Vec2,
}

pub(super) struct Block {
    pub kind: BlockKind,
    pub rect: Rect,
    /// Text lines, already cut to the width of the box.
    pub lines: Vec<(String, LineStyle)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Thread(usize),
    Resource(usize),
}

#[derive(Clone, Copy)]
pub(super) enum LineStyle {
    Title,
    Normal,
    Weak,
}

/// An arrow between two boxes, for painting and hit testing.
pub(super) struct Arrow {
    /// The curve as a line.
    pub points: Vec<Pos2>,
    pub color: Color32,
    pub width: f32,
    pub dashed: bool,
    pub head: bool,
    pub label: Option<String>,
    pub tooltip: String,
    pub ends: (BlockKind, BlockKind),
}

pub fn layout(project: &Project, program: &Program) -> GraphLayout {
    let (mut boxes, max_depth) = thread_blocks(project, program);
    let x = MARGIN + (max_depth + 1) as f32 * (BOX_W + COL_GAP);
    let resources = resource_blocks(project, program, &boxes, x);
    boxes.extend(resources);
    let max = boxes.iter().fold(Pos2::ZERO, |m, b| m.max(b.rect.max));
    let mut arrows = link_arrows(project, program, &boxes);
    arrows.extend(resource_arrows(project, program, &boxes));
    GraphLayout {
        boxes,
        arrows,
        size: max.to_vec2() + Vec2::splat(MARGIN),
    }
}

/// Spawn depth of every thread: the shortest chain of starts from the main
/// thread (`usize::MAX` if it is not reached).
fn spawn_depths(program: &Program) -> Vec<usize> {
    let n = program.threads.len();
    let mut depth = vec![usize::MAX; n];
    if n > 0 {
        depth[0] = 0;
    }
    for _ in 0..n {
        for (i, t) in program.threads.iter().enumerate() {
            for &p in &t.spawned_by {
                if depth[p] != usize::MAX {
                    depth[i] = depth[i].min(depth[p] + 1);
                }
            }
        }
    }
    depth
}

/// One box per thread, in columns by spawn depth; also returns the last column.
fn thread_blocks(project: &Project, program: &Program) -> (Vec<Block>, usize) {
    let depth = spawn_depths(program);
    let mut boxes = Vec::new();
    let mut column_y: HashMap<usize, f32> = HashMap::new();
    let mut max_depth = 0;
    for (i, t) in program.threads.iter().enumerate() {
        let d = if depth[i] == usize::MAX { 1 } else { depth[i] };
        max_depth = max_depth.max(d);
        let lines = thread_lines(project, t);
        let h = lines.len() as f32 * LINE + 2.0 * PAD;
        let y = column_y.entry(d).or_insert(MARGIN);
        let x = MARGIN + d as f32 * (BOX_W + COL_GAP);
        boxes.push(Block {
            kind: BlockKind::Thread(i),
            rect: Rect::from_min_size(pos2(x, *y), vec2(BOX_W, h)),
            lines,
        });
        *y += h + ROW_GAP;
    }
    (boxes, max_depth)
}

/// The text in the box of a thread.
fn thread_lines(project: &Project, t: &Thread) -> Vec<(String, LineStyle)> {
    let mut lines = vec![(clip(t.title()), LineStyle::Title)];
    if let Some((f, node)) = t.site {
        let line = node_line(project, f, node);
        let text = format!("started in {}:{line}", short_path(project, f));
        lines.push((clip(text), LineStyle::Weak));
    }
    let entries: Vec<String> = t
        .entries
        .iter()
        .take(MAX_NAMES)
        .map(|&f| project.function(f).name.clone())
        .collect();
    if !entries.is_empty() {
        let more = if t.entries.len() > MAX_NAMES {
            ", …"
        } else {
            ""
        };
        let text = format!("→ {}{more}", entries.join(", "));
        lines.push((clip(text), LineStyle::Normal));
    }
    if !t.callbacks.is_empty() {
        let cb: Vec<String> = t
            .callbacks
            .iter()
            .take(MAX_NAMES)
            .map(|&f| method_name(project, f))
            .collect();
        let text = format!("callbacks: {}", cb.join(", "));
        lines.push((clip(text), LineStyle::Weak));
    }
    let text = format!("{} functions", t.runs.len());
    lines.push((clip(text), LineStyle::Weak));
    lines
}

/// `Abfrage::starten` or `lkg::starten`: enough to tell same-named places apart.
fn short_path(project: &Project, f: FnId) -> String {
    let q = &project.function(f).qual_path;
    let parts: Vec<&str> = q.split("::").collect();
    parts[parts.len().saturating_sub(3)..].join("::")
}

/// `Type::method` for methods, the plain name for free functions.
fn method_name(project: &Project, f: FnId) -> String {
    let func = project.function(f);
    match func.owner.group_label() {
        Some(_) => {
            let parts: Vec<&str> = func.qual_path.split("::").collect();
            parts[parts.len().saturating_sub(2)..].join("::")
        }
        None => func.name.clone(),
    }
}

/// Cuts a line to what fits into a box.
fn clip(text: String) -> String {
    if text.chars().count() > MAX_LINE_CHARS {
        text.chars().take(MAX_LINE_CHARS - 1).chain(['…']).collect()
    } else {
        text
    }
}

/// Shared state in its own column at `x`, each object at the height of the
/// threads using it (keeps the dotted lines short).
fn resource_blocks(project: &Project, program: &Program, threads: &[Block], x: f32) -> Vec<Block> {
    let thread_center = |t: usize| {
        threads
            .iter()
            .find(|b| b.kind == BlockKind::Thread(t))
            .map_or(MARGIN, |b| b.rect.center().y)
    };
    let mut order: Vec<(usize, f32)> = program
        .resources
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // The main thread uses nearly everything; place by the other users.
            let others: Vec<usize> = r.users.iter().copied().filter(|&u| u != 0).collect();
            let users = if others.is_empty() { &r.users } else { &others };
            let ys: Vec<f32> = users.iter().map(|&u| thread_center(u)).collect();
            (i, ys.iter().sum::<f32>() / ys.len().max(1) as f32)
        })
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut boxes = Vec::new();
    let mut y = MARGIN;
    for (i, center) in order {
        let r = &program.resources[i];
        let lines = vec![
            (clip(format!("{} ({})", r.name, r.kind)), LineStyle::Title),
            (
                clip(format!("created in {}", short_path(project, r.site.0))),
                LineStyle::Weak,
            ),
            (clip(format!("{} threads", r.users.len())), LineStyle::Weak),
        ];
        let h = lines.len() as f32 * LINE + 2.0 * PAD;
        y = y.max(center - h / 2.0);
        boxes.push(Block {
            kind: BlockKind::Resource(i),
            rect: Rect::from_min_size(pos2(x, y), vec2(BOX_W * 0.8, h)),
            lines,
        });
        y += h + ROW_GAP / 2.0;
    }
    boxes
}

fn rect_of(boxes: &[Block], kind: BlockKind) -> Option<Rect> {
    boxes.iter().find(|b| b.kind == kind).map(|b| b.rect)
}

/// `path::of::function:line` of a statement.
fn site_text(project: &Project, (f, node): (FnId, NodeId)) -> String {
    let line = node_line(project, f, node);
    format!("{}:{line}", project.function(f).qual_path)
}

/// Arrows between threads: starts and channels.
fn link_arrows(project: &Project, program: &Program, boxes: &[Block]) -> Vec<Arrow> {
    let mut out = Vec::new();
    // Several channels between the same pair are fanned out.
    let mut pair_count: HashMap<(usize, usize), usize> = HashMap::new();
    for l in &program.links {
        let (Some(a), Some(b)) = (
            rect_of(boxes, BlockKind::Thread(l.from)),
            rect_of(boxes, BlockKind::Thread(l.to)),
        ) else {
            continue;
        };
        let k = pair_count
            .entry((l.from.min(l.to), l.from.max(l.to)))
            .or_insert(0);
        let offset = *k as f32 * FAN_OUT;
        *k += 1;
        let site = site_text(project, l.site);
        let (color, width, label, tooltip) = match &l.kind {
            LinkKind::Spawn => (
                SPAWN,
                1.5,
                None,
                format!("starts {}\nat {site}", program.threads[l.to].title()),
            ),
            LinkKind::Channel { name, api } => (
                CHANNEL,
                3.0,
                Some(name.clone()),
                format!("channel {name} ({api})\ncreated at {site}"),
            ),
        };
        out.push(Arrow {
            points: bezier(&connect(a, b, offset), CURVE_STEPS),
            color,
            width,
            dashed: false,
            head: true,
            label,
            tooltip,
            ends: (BlockKind::Thread(l.from), BlockKind::Thread(l.to)),
        });
    }
    out
}

/// Dashed lines from every thread to the shared state it uses.
fn resource_arrows(project: &Project, program: &Program, boxes: &[Block]) -> Vec<Arrow> {
    let mut out = Vec::new();
    for (ri, r) in program.resources.iter().enumerate() {
        let Some(rb) = rect_of(boxes, BlockKind::Resource(ri)) else {
            continue;
        };
        for &u in &r.users {
            let Some(ub) = rect_of(boxes, BlockKind::Thread(u)) else {
                continue;
            };
            out.push(Arrow {
                points: bezier(&connect(ub, rb, 0.0), CURVE_STEPS),
                color: SHARED,
                width: 1.5,
                dashed: true,
                head: false,
                label: None,
                tooltip: format!(
                    "{} uses shared {} ({})\ncreated at {}",
                    program.threads[u].title(),
                    r.name,
                    r.kind,
                    site_text(project, r.site)
                ),
                ends: (BlockKind::Thread(u), BlockKind::Resource(ri)),
            });
        }
    }
    out
}

/// Bézier from box `a` to box `b`, leaving and entering on the facing sides.
fn connect(a: Rect, b: Rect, offset: f32) -> [Pos2; 4] {
    let ay = a.min.y + (LINE + PAD).min(a.height() / 2.0) + offset;
    let by = b.min.y + (LINE + PAD).min(b.height() / 2.0) + offset;
    if b.min.x > a.max.x {
        let (p0, p3) = (pos2(a.max.x, ay), pos2(b.min.x, by));
        let dx = (p3.x - p0.x) * 0.5;
        [p0, p0 + vec2(dx, 0.0), p3 - vec2(dx, 0.0), p3]
    } else if b.max.x < a.min.x {
        let (p0, p3) = (pos2(a.min.x, ay), pos2(b.max.x, by));
        let dx = (p0.x - p3.x) * 0.5;
        [p0, p0 - vec2(dx, 0.0), p3 + vec2(dx, 0.0), p3]
    } else {
        // Same column: arc out to the right.
        let (p0, p3) = (pos2(a.max.x, ay), pos2(b.max.x, by));
        let out = 40.0 + 0.15 * (p3.y - p0.y).abs() + offset;
        let x = p0.x.max(p3.x) + out;
        [p0, pos2(x, p0.y), pos2(x, p3.y), p3]
    }
}
