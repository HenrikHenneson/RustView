//! First pass, bottom-up: the size every node needs.

use emath::{Vec2, vec2};
use rv_ir::{NodeId, NodeKind};

use crate::ctx::{Ctx, mode_text};
use crate::{BORDER, FRAME_PAD, GAP, PAD};

impl Ctx<'_> {
    /// Measures `id` and everything shown inside it; the sizes are kept for
    /// placing.
    pub(crate) fn measure(&mut self, id: NodeId) -> Vec2 {
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

    fn header_width(&self, id: NodeId) -> f32 {
        let texts = self.struct_texts(id);
        let gap = self.m.char_width;
        let mut w = PAD + self.m.icon() + gap + self.m.text(&texts.title) + PAD;
        if let Some(p) = &texts.prefix {
            w += self.m.text(p) + gap;
        }
        if let Some(n) = self.notes.get(&id) {
            w += 2.0 * gap + self.m.text(n);
        }
        if let Some(i) = &texts.info {
            w += 2.0 * gap + self.m.text(i);
        }
        if let Some(cases) = self.cases(id) {
            w += 2.0 * gap + self.m.text(mode_text(cases.showing_all));
            if !cases.showing_all {
                let count = cases.count;
                w += 2.0 * self.m.icon() + self.m.text(&format!("{count}/{count}")) + 2.0 * gap;
            }
        }
        w
    }

    fn measure_structure(&mut self, id: NodeId) -> Vec2 {
        let header = vec2(self.header_width(id), self.row());
        if !self.expanded(id) {
            return header;
        }
        let frames = self.state.visible_frames(self.body, id);
        let sizes: Vec<Vec2> = frames.iter().map(|&f| self.measure(f)).collect();
        // Cases run across the statement direction.
        let inner = if self.vertical {
            side_by_side(&sizes)
        } else {
            stacked(&sizes)
        };
        vec2(
            header.x.max(inner.x + 2.0 * BORDER),
            header.y + inner.y + BORDER,
        )
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
        let mut content = if sizes.is_empty() {
            vec2(self.m.text("(empty)"), self.m.line_height)
        } else if self.vertical {
            stacked(&sizes)
        } else {
            side_by_side(&sizes)
        };
        content.x = content.x.max(title_w);
        vec2(
            content.x + 2.0 * FRAME_PAD,
            content.y + title_h + 2.0 * FRAME_PAD,
        )
    }
}

/// Space between `count` boxes in a row or column.
fn gaps(count: usize) -> f32 {
    GAP * count.saturating_sub(1) as f32
}

/// Size of boxes next to each other, left to right.
fn side_by_side(sizes: &[Vec2]) -> Vec2 {
    vec2(
        sizes.iter().map(|s| s.x).sum::<f32>() + gaps(sizes.len()),
        sizes.iter().map(|s| s.y).fold(0.0, f32::max),
    )
}

/// Size of boxes below each other.
fn stacked(sizes: &[Vec2]) -> Vec2 {
    vec2(
        sizes.iter().map(|s| s.x).fold(0.0, f32::max),
        sizes.iter().map(|s| s.y).sum::<f32>() + gaps(sizes.len()),
    )
}
