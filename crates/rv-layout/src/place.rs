//! Second pass, top-down: every visible node gets its rectangle and the
//! positions of its texts and controls.

use emath::{Rect, Vec2, pos2, vec2};
use rv_ir::{NodeId, NodeKind};

use crate::ctx::{Cases, Ctx, mode_text};
use crate::metrics::Metrics;
use crate::{BORDER, FRAME_PAD, GAP, Item, ItemKind, PAD, Selector, StructItem, Text};

/// Texts of a header line, placed one after the other from left to right.
struct TextRow {
    x: f32,
    y: f32,
    /// Space after every text.
    gap: f32,
    m: Metrics,
}

impl TextRow {
    fn text(&mut self, s: String) -> Text {
        let r = Rect::from_min_size(
            pos2(self.x, self.y),
            vec2(self.m.text(&s), self.m.line_height),
        );
        self.x = r.max.x + self.gap;
        Text { text: s, rect: r }
    }

    /// Like [`Self::text`], set off from what is before it by another gap.
    fn text_apart(&mut self, s: String) -> Text {
        self.x += self.gap;
        self.text(s)
    }
}

impl Ctx<'_> {
    fn push(&mut self, item: Item) {
        self.index.insert(item.node, self.items.len());
        self.items.push(item);
    }

    /// `rect` may be larger than the measured size (stretched frames).
    pub(crate) fn place(&mut self, id: NodeId, rect: Rect, depth: u32) {
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
        let toggle = Rect::from_min_size(
            header.min + vec2(PAD, PAD),
            vec2(self.m.icon(), self.m.line_height),
        );
        let texts = self.struct_texts(id);
        let gap = self.m.char_width;
        let mut row = TextRow {
            x: toggle.max.x + gap,
            y: header.min.y + PAD,
            gap,
            m: self.m,
        };
        let prefix = texts.prefix.map(|p| row.text(p));
        let title = row.text(texts.title);
        let note = self.notes.get(&id).map(|n| row.text_apart(n.clone()));
        let info = texts.info.map(|i| row.text_apart(i));
        let selector = self.cases(id).map(|cases| self.selector(cases, header));

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
        if expanded {
            let inner = Rect::from_min_max(
                pos2(rect.min.x + BORDER, header.max.y),
                pos2(rect.max.x - BORDER, rect.max.y - BORDER),
            );
            self.place_frames(id, inner, depth);
        }
    }

    /// Case selector, right-aligned in the header.
    fn selector(&self, cases: Cases, header: Rect) -> Selector {
        let icon = self.m.icon();
        let gap = self.m.char_width;
        let y = header.min.y + PAD;
        let mut right = header.max.x - PAD;
        let mode_s = mode_text(cases.showing_all).to_string();
        let mode_w = self.m.text(&mode_s);
        let mode = Text {
            rect: Rect::from_min_size(pos2(right - mode_w, y), vec2(mode_w, self.m.line_height)),
            text: mode_s,
        };
        right = mode.rect.min.x - 2.0 * gap;
        let (mut prev, mut position, mut next) = (None, None, None);
        if !cases.showing_all {
            let n = Rect::from_min_size(pos2(right - icon, y), vec2(icon, self.m.line_height));
            let pos_s = format!("{}/{}", cases.index + 1, cases.count);
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
            showing_all: cases.showing_all,
            index: cases.index,
            count: cases.count,
            prev,
            position,
            next,
            mode,
        }
    }

    /// Places the shown frames of structure `id` in `inner`, across the
    /// statement direction.
    fn place_frames(&mut self, id: NodeId, inner: Rect, depth: u32) {
        let frames = self.state.visible_frames(self.body, id);
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
