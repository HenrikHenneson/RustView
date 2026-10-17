//! Geometry for RustView structure diagrams.
//!
//! Sizes are computed bottom-up (a structure is as large as its frames, a
//! frame as large as its items), then everything is placed top-down. All
//! coordinates are in diagram units at zoom 1; the GUI scales and pans.

use std::collections::{HashMap, HashSet, VecDeque};

pub mod layered;

use emath::{Pos2, Rect, Vec2, pos2, vec2};
use rv_ir::{Body, ExitKind, FrameRole, NodeId, NodeKind};

/// Case structures with more frames show one case at a time by default.
pub const MAX_SIDE_BY_SIDE: usize = 4;

/// Padding inside statement boxes and structure headers.
const PAD: f32 = 4.0;
/// Padding inside frames.
const FRAME_PAD: f32 = 8.0;
/// Space between items of a frame and between frames.
const GAP: f32 = 8.0;
/// Width of a structure's border around its frames.
const BORDER: f32 = 4.0;
/// Space between operand column and operation in expression trees.
const EXPR_GAP_X: f32 = 16.0;
const EXPR_GAP_Y: f32 = 4.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Statements top to bottom, cases side by side.
    #[default]
    Vertical,
    /// Statements left to right, cases stacked (closer to LabVIEW).
    Horizontal,
}

/// Font metrics of the (monospace) label font at zoom 1.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub char_width: f32,
    pub line_height: f32,
    /// Longer labels are cut with `…`.
    pub max_label_chars: usize,
}

impl Metrics {
    fn text(&self, s: &str) -> f32 {
        s.chars().count() as f32 * self.char_width
    }

    /// Width of the expand/collapse triangle and the case arrows.
    fn icon(&self) -> f32 {
        self.line_height
    }
}

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
    let mut visible = 0;
    let mut queue = VecDeque::new();
    let open = |s: NodeId, queue: &mut VecDeque<NodeId>| -> usize {
        let mut cost = 0;
        for f in state.visible_frames(body, s) {
            for &item in &body.node(f).children {
                cost += 1;
                match body.node(item).kind {
                    NodeKind::Structure { .. } => queue.push_back(item),
                    _ => queue.extend(body.node(item).children.iter().copied()),
                }
            }
        }
        cost
    };
    visible += open(body.root, &mut queue);
    while let Some(s) = queue.pop_front() {
        let mut probe = VecDeque::new();
        let cost = open(s, &mut probe);
        if visible + cost <= budget {
            visible += cost;
            expanded.insert(s);
            queue.extend(probe);
        }
    }
    expanded
}

#[derive(Clone, Debug)]
pub struct Layout {
    /// Parents before children (paint order).
    pub items: Vec<Item>,
    pub size: Vec2,
    index: HashMap<NodeId, usize>,
    /// Expression nodes of statements in expression mode.
    pub exprs: Vec<ExprItem>,
    /// Operand -> operation, as (from, to) points.
    pub expr_wires: Vec<(Pos2, Pos2)>,
}

/// One node of a statement's expression tree.
#[derive(Clone, Debug)]
pub struct ExprItem {
    pub stmt: NodeId,
    /// Index into the statement's `ExprTree::nodes`.
    pub index: usize,
    pub rect: Rect,
    pub text: Text,
}

impl Layout {
    pub fn item(&self, node: NodeId) -> Option<&Item> {
        self.index.get(&node).map(|&i| &self.items[i])
    }

    /// Innermost item at `pos`.
    pub fn hit(&self, pos: Pos2) -> Option<&Item> {
        self.items.iter().rev().find(|i| i.rect.contains(pos))
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub node: NodeId,
    pub rect: Rect,
    /// Nesting depth in structures (root = 0).
    pub depth: u32,
    pub kind: ItemKind,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Structure(Box<StructItem>),
    Frame {
        /// Case pattern / `else` / `cond`; `None` for plain bodies.
        title: Option<Text>,
    },
    Stmt {
        text: Text,
        /// `?2`, `=>` (value) etc.
        badges: Option<Text>,
        /// Annotation from the caller (e.g. "thread 'worker'").
        note: Option<Text>,
    },
}

#[derive(Clone, Debug)]
pub struct StructItem {
    pub expanded: bool,
    pub header: Rect,
    pub toggle: Rect,
    pub prefix: Option<Text>,
    pub title: Text,
    /// Annotation from the caller (e.g. "thread 'worker'").
    pub note: Option<Text>,
    /// Exits to this structure, or the size when collapsed.
    pub info: Option<Text>,
    pub selector: Option<Selector>,
}

/// Controls of a case structure.
#[derive(Clone, Debug)]
pub struct Selector {
    pub showing_all: bool,
    pub index: usize,
    pub count: usize,
    /// Arrows and position; only in one-at-a-time mode.
    pub prev: Option<Rect>,
    pub position: Option<Text>,
    pub next: Option<Rect>,
    /// Switches between "all" and "one".
    pub mode: Text,
}

/// A piece of text and where it goes (top-left at `rect.min`).
#[derive(Clone, Debug)]
pub struct Text {
    pub text: String,
    pub rect: Rect,
}

/// Short annotations shown next to a node's label (lane badges etc.).
pub type Notes = HashMap<NodeId, String>;

pub fn layout(
    body: &Body,
    state: &ViewState,
    defaults: &HashSet<NodeId>,
    orientation: Orientation,
    metrics: Metrics,
    notes: &Notes,
) -> Layout {
    let mut exits: HashMap<NodeId, ExitCounts> = HashMap::new();
    for e in &body.exits {
        if let Some(t) = e.target {
            exits.entry(t).or_default().add(&e.kind);
        }
    }
    let mut ctx = Ctx {
        body,
        state,
        defaults,
        vertical: orientation == Orientation::Vertical,
        m: metrics,
        sizes: vec![Vec2::ZERO; body.nodes.len()],
        stmt_counts: stmt_counts(body),
        exits,
        notes,
        items: Vec::new(),
        index: HashMap::new(),
        exprs: Vec::new(),
        expr_wires: Vec::new(),
    };
    let size = ctx.measure(body.root);
    ctx.place(body.root, Rect::from_min_size(Pos2::ZERO, size), 0);
    Layout {
        items: ctx.items,
        size,
        index: ctx.index,
        exprs: ctx.exprs,
        expr_wires: ctx.expr_wires,
    }
}

#[derive(Default)]
struct ExitCounts {
    returns: u32,
    tries: u32,
    breaks: u32,
    continues: u32,
}

impl ExitCounts {
    fn add(&mut self, kind: &ExitKind) {
        match kind {
            ExitKind::Return => self.returns += 1,
            ExitKind::Try => self.tries += 1,
            ExitKind::Break { .. } => self.breaks += 1,
            ExitKind::Continue { .. } => self.continues += 1,
        }
    }

    fn summary(&self) -> Option<String> {
        let parts: Vec<String> = [
            ("return", self.returns),
            ("?", self.tries),
            ("break", self.breaks),
            ("continue", self.continues),
        ]
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(what, n)| format!("{what}×{n}"))
        .collect();
        (!parts.is_empty()).then(|| format!("exits: {}", parts.join(" ")))
    }
}

fn stmt_counts(body: &Body) -> Vec<usize> {
    let mut counts = vec![0; body.nodes.len()];
    // Children always have higher ids than their parents.
    for i in (0..body.nodes.len()).rev() {
        let node = &body.nodes[i];
        let own = usize::from(matches!(node.kind, NodeKind::Stmt { .. }));
        counts[i] += own;
        if let Some(p) = node.parent {
            counts[p.index()] += counts[i];
        }
    }
    counts
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

struct Ctx<'a> {
    body: &'a Body,
    state: &'a ViewState,
    defaults: &'a HashSet<NodeId>,
    vertical: bool,
    m: Metrics,
    sizes: Vec<Vec2>,
    stmt_counts: Vec<usize>,
    exits: HashMap<NodeId, ExitCounts>,
    notes: &'a Notes,
    items: Vec<Item>,
    exprs: Vec<ExprItem>,
    expr_wires: Vec<(Pos2, Pos2)>,
    index: HashMap<NodeId, usize>,
}

impl<'a> Ctx<'a> {
    fn expanded(&self, id: NodeId) -> bool {
        self.state.is_expanded(self.defaults, id)
    }

    fn row(&self) -> f32 {
        self.m.line_height + 2.0 * PAD
    }

    // ----- texts -------------------------------------------------------------

    fn stmt_text(&self, id: NodeId) -> String {
        truncate(&self.body.node(id).label, self.m.max_label_chars)
    }

    fn stmt_badges(&self, id: NodeId) -> Option<String> {
        let node = self.body.node(id);
        let mut b = Vec::new();
        if node.tries > 0 {
            b.push(if node.tries == 1 {
                "?".to_string()
            } else {
                format!("?{}", node.tries)
            });
        }
        if node.value {
            b.push("=>".to_string());
        }
        (!b.is_empty()).then(|| b.join(" "))
    }

    fn struct_texts(&self, id: NodeId) -> (Option<String>, String, Option<String>) {
        let node = self.body.node(id);
        let NodeKind::Structure { prefix, .. } = &node.kind else {
            unreachable!()
        };
        let title = truncate(&node.label, self.m.max_label_chars);
        let mut info = Vec::new();
        if node.tries > 0 {
            info.push(format!("?{}", node.tries));
        }
        if !self.expanded(id) {
            info.push(format!("{} stmts", self.stmt_counts[id.index()]));
        }
        if let Some(e) = self.exits.get(&id).and_then(ExitCounts::summary) {
            info.push(e);
        }
        let info = (!info.is_empty()).then(|| info.join("  ·  "));
        (prefix.clone(), title, info)
    }

    /// Text of the case selector, if the structure has alternatives.
    fn selector_texts(&self, id: NodeId) -> Option<(bool, usize, usize)> {
        let count = case_frames(self.body, id).len();
        if count < 2 || !self.expanded(id) {
            return None;
        }
        let all = self.state.shows_all_cases(self.body, id);
        Some((all, self.state.case_index(self.body, id), count))
    }

    fn header_width(&self, id: NodeId) -> f32 {
        let (prefix, title, info) = self.struct_texts(id);
        let gap = self.m.char_width;
        let mut w = PAD + self.m.icon() + gap + self.m.text(&title) + PAD;
        if let Some(p) = &prefix {
            w += self.m.text(p) + gap;
        }
        if let Some(n) = self.notes.get(&id) {
            w += 2.0 * gap + self.m.text(n);
        }
        if let Some(i) = &info {
            w += 2.0 * gap + self.m.text(i);
        }
        if let Some((all, _, count)) = self.selector_texts(id) {
            w += 2.0 * gap + self.m.text(mode_text(all));
            if !all {
                w += 2.0 * self.m.icon() + self.m.text(&format!("{count}/{count}")) + 2.0 * gap;
            }
        }
        w
    }

    // ----- measuring ---------------------------------------------------------

    fn measure(&mut self, id: NodeId) -> Vec2 {
        let size = match &self.body.node(id).kind {
            NodeKind::Stmt { .. } => self.measure_stmt(id),
            NodeKind::Structure { .. } => self.measure_structure(id),
            NodeKind::Frame { .. } => self.measure_frame(id),
        };
        self.sizes[id.index()] = size;
        size
    }

    fn measure_stmt(&mut self, id: NodeId) -> Vec2 {
        let mut w = self.m.text(&self.stmt_text(id)) + 2.0 * PAD;
        if let Some(b) = self.stmt_badges(id) {
            w += self.m.char_width + self.m.text(&b);
        }
        if let Some(n) = self.notes.get(&id) {
            w += 2.0 * self.m.char_width + self.m.text(n);
        }
        let mut h = self.row();
        if let Some(t) = self.expr_tree(id) {
            let size = self.tree_size(t, t.root);
            w = w.max(size.x + 2.0 * PAD);
            h += size.y + PAD;
        }
        for &c in &self.body.node(id).children.clone() {
            let s = self.measure(c);
            w = w.max(s.x + 2.0 * PAD);
            h += s.y + PAD;
        }
        vec2(w, h)
    }

    /// The statement's expression tree, if its structure shows expressions.
    fn expr_tree(&self, id: NodeId) -> Option<&'a rv_ir::ExprTree> {
        let body: &'a Body = self.body;
        let tree = body.node(id).expr.as_ref()?;
        // Trivial trees (a single call without operands) add nothing.
        if tree.nodes.len() < 2 {
            return None;
        }
        let mut cur = body.node(id).parent;
        while let Some(n) = cur {
            if self.state.expressions.contains(&n) {
                return Some(tree);
            }
            cur = body.node(n).parent;
        }
        None
    }

    fn expr_box(&self, text: &str) -> Vec2 {
        vec2(self.m.text(text) + 2.0 * PAD, self.m.line_height + PAD)
    }

    /// Size of a subtree: operands in a column left of their operation.
    fn tree_size(&self, t: &rv_ir::ExprTree, i: usize) -> Vec2 {
        let node = &t.nodes[i];
        let own = self.expr_box(&node.text);
        if node.children.is_empty() {
            return own;
        }
        let kids: Vec<Vec2> = node
            .children
            .iter()
            .map(|&c| self.tree_size(t, c))
            .collect();
        let kids_w = kids.iter().map(|k| k.x).fold(0.0, f32::max);
        let kids_h = kids.iter().map(|k| k.y).sum::<f32>() + EXPR_GAP_Y * (kids.len() - 1) as f32;
        vec2(kids_w + EXPR_GAP_X + own.x, own.y.max(kids_h))
    }

    /// Places a subtree in `area`; returns the operation's box.
    fn place_tree(&mut self, stmt: NodeId, t: &rv_ir::ExprTree, i: usize, min: Pos2) -> Rect {
        let node = &t.nodes[i];
        let size = self.tree_size(t, i);
        let own_size = self.expr_box(&node.text);
        let own = Rect::from_min_size(
            pos2(
                min.x + size.x - own_size.x,
                min.y + (size.y - own_size.y) / 2.0,
            ),
            own_size,
        );
        let mut y = min.y;
        if !node.children.is_empty() {
            let kids_h: f32 = node
                .children
                .iter()
                .map(|&c| self.tree_size(t, c).y)
                .sum::<f32>()
                + EXPR_GAP_Y * (node.children.len() - 1) as f32;
            y += (size.y - kids_h) / 2.0;
        }
        for &c in &node.children {
            let ks = self.tree_size(t, c);
            let child = self.place_tree(stmt, t, c, pos2(min.x, y));
            self.expr_wires
                .push((child.right_center(), own.left_center()));
            y += ks.y + EXPR_GAP_Y;
        }
        self.exprs.push(ExprItem {
            stmt,
            index: i,
            rect: own,
            text: Text {
                text: node.text.clone(),
                rect: Rect::from_min_size(
                    own.min + vec2(PAD, PAD / 2.0),
                    vec2(self.m.text(&node.text), self.m.line_height),
                ),
            },
        });
        own
    }

    fn measure_structure(&mut self, id: NodeId) -> Vec2 {
        let header = vec2(self.header_width(id), self.row());
        if !self.expanded(id) {
            return header;
        }
        let frames = self.state.visible_frames(self.body, id);
        let sizes: Vec<Vec2> = frames.iter().map(|&f| self.measure(f)).collect();
        let gaps = GAP * sizes.len().saturating_sub(1) as f32;
        let inner = if self.vertical {
            vec2(
                sizes.iter().map(|s| s.x).sum::<f32>() + gaps,
                sizes.iter().map(|s| s.y).fold(0.0, f32::max),
            )
        } else {
            vec2(
                sizes.iter().map(|s| s.x).fold(0.0, f32::max),
                sizes.iter().map(|s| s.y).sum::<f32>() + gaps,
            )
        };
        vec2(
            header.x.max(inner.x + 2.0 * BORDER),
            header.y + inner.y + BORDER,
        )
    }

    fn frame_title(&self, id: NodeId) -> Option<String> {
        let node = self.body.node(id);
        match &node.kind {
            NodeKind::Frame {
                role: FrameRole::Body,
            } => None,
            NodeKind::Frame { .. } => Some(truncate(&node.label, self.m.max_label_chars)),
            _ => None,
        }
    }

    fn measure_frame(&mut self, id: NodeId) -> Vec2 {
        let title_h = if self.frame_title(id).is_some() {
            self.row()
        } else {
            0.0
        };
        let title_w = self
            .frame_title(id)
            .map_or(0.0, |t| self.m.text(&t) + 2.0 * PAD);
        let children = self.body.node(id).children.clone();
        let sizes: Vec<Vec2> = children.iter().map(|&c| self.measure(c)).collect();
        let gaps = GAP * sizes.len().saturating_sub(1) as f32;
        let mut content = if sizes.is_empty() {
            vec2(self.m.text("(empty)"), self.m.line_height)
        } else if self.vertical {
            vec2(
                sizes.iter().map(|s| s.x).fold(0.0, f32::max),
                sizes.iter().map(|s| s.y).sum::<f32>() + gaps,
            )
        } else {
            vec2(
                sizes.iter().map(|s| s.x).sum::<f32>() + gaps,
                sizes.iter().map(|s| s.y).fold(0.0, f32::max),
            )
        };
        content.x = content.x.max(title_w);
        vec2(
            content.x + 2.0 * FRAME_PAD,
            content.y + title_h + 2.0 * FRAME_PAD,
        )
    }

    // ----- placing -----------------------------------------------------------

    fn push(&mut self, item: Item) {
        self.index.insert(item.node, self.items.len());
        self.items.push(item);
    }

    /// `rect` may be larger than the measured size (stretched frames).
    fn place(&mut self, id: NodeId, rect: Rect, depth: u32) {
        match &self.body.node(id).kind {
            NodeKind::Stmt { .. } => self.place_stmt(id, rect, depth),
            NodeKind::Structure { .. } => self.place_structure(id, rect, depth),
            NodeKind::Frame { .. } => self.place_frame(id, rect, depth),
        }
    }

    fn place_stmt(&mut self, id: NodeId, rect: Rect, depth: u32) {
        let text = self.stmt_text(id);
        let text_pos = rect.min + vec2(PAD, PAD);
        let text_rect = Rect::from_min_size(text_pos, vec2(self.m.text(&text), self.m.line_height));
        let badges = self.stmt_badges(id).map(|b| {
            let pos = pos2(text_rect.max.x + self.m.char_width, text_pos.y);
            Text {
                rect: Rect::from_min_size(pos, vec2(self.m.text(&b), self.m.line_height)),
                text: b,
            }
        });
        let note_x =
            badges.as_ref().map_or(text_rect.max.x, |b| b.rect.max.x) + 2.0 * self.m.char_width;
        let note = self.notes.get(&id).map(|n| Text {
            rect: Rect::from_min_size(
                pos2(note_x, text_pos.y),
                vec2(self.m.text(n), self.m.line_height),
            ),
            text: n.clone(),
        });
        self.push(Item {
            node: id,
            rect,
            depth,
            kind: ItemKind::Stmt {
                text: Text {
                    text,
                    rect: text_rect,
                },
                badges,
                note,
            },
        });
        let mut y = rect.min.y + self.row();
        if let Some(t) = self.expr_tree(id) {
            let size = self.tree_size(t, t.root);
            self.place_tree(id, t, t.root, pos2(rect.min.x + PAD, y));
            y += size.y + PAD;
        }
        for c in self.body.node(id).children.clone() {
            let size = self.sizes[c.index()];
            self.place(
                c,
                Rect::from_min_size(pos2(rect.min.x + PAD, y), size),
                depth + 1,
            );
            y += size.y + PAD;
        }
    }

    fn place_structure(&mut self, id: NodeId, rect: Rect, depth: u32) {
        let expanded = self.expanded(id);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), self.row()));
        let icon = self.m.icon();
        let gap = self.m.char_width;
        let toggle =
            Rect::from_min_size(header.min + vec2(PAD, PAD), vec2(icon, self.m.line_height));
        let (prefix, title, info) = self.struct_texts(id);
        let mut x = toggle.max.x + gap;
        let y = header.min.y + PAD;
        let text_at = |s: String, x: &mut f32, m: &Metrics| {
            let r = Rect::from_min_size(pos2(*x, y), vec2(m.text(&s), m.line_height));
            *x = r.max.x + gap;
            Text { text: s, rect: r }
        };
        let prefix = prefix.map(|p| text_at(p, &mut x, &self.m));
        let title = text_at(title, &mut x, &self.m);
        let note = self.notes.get(&id).map(|n| {
            x += gap;
            text_at(n.clone(), &mut x, &self.m)
        });
        let info = info.map(|i| {
            x += gap;
            text_at(i, &mut x, &self.m)
        });

        // Case selector, right-aligned in the header.
        let selector = self.selector_texts(id).map(|(all, index, count)| {
            let mut right = header.max.x - PAD;
            let mode_s = mode_text(all).to_string();
            let mode_w = self.m.text(&mode_s);
            let mode = Text {
                rect: Rect::from_min_size(
                    pos2(right - mode_w, y),
                    vec2(mode_w, self.m.line_height),
                ),
                text: mode_s,
            };
            right = mode.rect.min.x - 2.0 * gap;
            let (mut prev, mut position, mut next) = (None, None, None);
            if !all {
                let n = Rect::from_min_size(pos2(right - icon, y), vec2(icon, self.m.line_height));
                let pos_s = format!("{}/{count}", index + 1);
                let pos_w = self.m.text(&pos_s);
                let p_rect = Rect::from_min_size(
                    pos2(n.min.x - gap - pos_w, y),
                    vec2(pos_w, self.m.line_height),
                );
                let p = Rect::from_min_size(
                    pos2(p_rect.min.x - gap - icon, y),
                    vec2(icon, self.m.line_height),
                );
                prev = Some(p);
                position = Some(Text {
                    text: pos_s,
                    rect: p_rect,
                });
                next = Some(n);
            }
            Selector {
                showing_all: all,
                index,
                count,
                prev,
                position,
                next,
                mode,
            }
        });

        self.push(Item {
            node: id,
            rect,
            depth,
            kind: ItemKind::Structure(Box::new(StructItem {
                expanded,
                header,
                toggle,
                prefix,
                title,
                note,
                info,
                selector,
            })),
        });
        if !expanded {
            return;
        }

        let frames = self.state.visible_frames(self.body, id);
        let inner = Rect::from_min_max(
            pos2(rect.min.x + BORDER, header.max.y),
            pos2(rect.max.x - BORDER, rect.max.y - BORDER),
        );
        let sizes: Vec<Vec2> = frames.iter().map(|f| self.sizes[f.index()]).collect();
        let n = frames.len();
        if self.vertical {
            // Side by side; equal heights; the last frame takes leftover width.
            let used: f32 =
                sizes.iter().map(|s| s.x).sum::<f32>() + GAP * n.saturating_sub(1) as f32;
            let extra = (inner.width() - used).max(0.0);
            let mut x = inner.min.x;
            for (i, (&f, s)) in frames.iter().zip(&sizes).enumerate() {
                let w = s.x + if i + 1 == n { extra } else { 0.0 };
                self.place(
                    f,
                    Rect::from_min_size(pos2(x, inner.min.y), vec2(w, inner.height())),
                    depth,
                );
                x += w + GAP;
            }
        } else {
            // Stacked; equal widths.
            let mut y = inner.min.y;
            for (&f, s) in frames.iter().zip(&sizes) {
                self.place(
                    f,
                    Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), s.y)),
                    depth,
                );
                y += s.y + GAP;
            }
        }
    }

    fn place_frame(&mut self, id: NodeId, rect: Rect, depth: u32) {
        let title = self.frame_title(id).map(|t| Text {
            rect: Rect::from_min_size(
                rect.min + vec2(FRAME_PAD, FRAME_PAD),
                vec2(self.m.text(&t), self.m.line_height),
            ),
            text: t,
        });
        let title_h = if title.is_some() { self.row() } else { 0.0 };
        self.push(Item {
            node: id,
            rect,
            depth,
            kind: ItemKind::Frame { title },
        });
        let mut pos = rect.min + vec2(FRAME_PAD, FRAME_PAD + title_h);
        for c in self.body.node(id).children.clone() {
            let size = self.sizes[c.index()];
            self.place(c, Rect::from_min_size(pos, size), depth + 1);
            if self.vertical {
                pos.y += size.y + GAP;
            } else {
                pos.x += size.x + GAP;
            }
        }
    }
}

fn mode_text(showing_all: bool) -> &'static str {
    if showing_all { "one" } else { "all" }
}

#[cfg(test)]
mod tests;
