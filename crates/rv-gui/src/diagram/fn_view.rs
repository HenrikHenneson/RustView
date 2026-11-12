//! How one function is shown: what is expanded, which cases and expressions
//! are visible, zoom and scroll position. All changes the user can make to a
//! diagram are methods here; the widgets only call them.

use std::collections::HashSet;

use egui::{Pos2, Vec2};
use rv_ir::{Body, NodeId, NodeKind};
use rv_layout::ViewState;

/// Items shown initially; larger functions start partly collapsed.
pub(super) const BUDGET: usize = 150;
pub(super) const MIN_ZOOM: f32 = 0.05;
pub(super) const MAX_ZOOM: f32 = 4.0;
/// Factor of one step of the zoom buttons.
const ZOOM_STEP: f32 = 1.25;
/// Space kept free at the left and top of the diagram.
pub(super) const MARGIN: f32 = 16.0;

/// A control in the header of a structure.
#[derive(Clone, Copy)]
pub(super) enum Control {
    Toggle,
    PrevCase,
    NextCase,
    CaseMode,
}

/// Per-function view state, kept while switching between functions.
pub(super) struct FnView {
    pub(super) state: ViewState,
    pub(super) defaults: HashSet<NodeId>,
    /// To detect that a reload changed the body (node ids are then meaningless).
    pub(super) node_count: usize,
    pub(super) zoom: f32,
    pub(super) pan: Vec2,
    /// Counts the changes that need a new layout.
    pub(super) version: u64,
}

impl FnView {
    pub(super) fn new(body: &Body, expressions: bool) -> Self {
        let mut state = ViewState::default();
        if expressions {
            state.expressions.insert(body.root);
        }
        FnView {
            defaults: rv_layout::default_expanded(body, &state, BUDGET),
            state,
            node_count: body.nodes.len(),
            zoom: 1.0,
            pan: Vec2::splat(MARGIN),
            version: 0,
        }
    }

    /// Expands all structures around `node` and picks the cases on the way,
    /// so that it is visible.
    pub(super) fn reveal(&mut self, body: &Body, node: NodeId) {
        let mut child = node;
        while let Some(parent) = body.node(child).parent {
            if matches!(body.node(parent).kind, NodeKind::Structure { .. }) {
                self.state.expanded.insert(parent, true);
                let cases = rv_layout::case_frames(body, parent);
                if let Some(i) = cases.iter().position(|&c| c == child) {
                    self.state.case_index.insert(parent, i);
                }
            }
            child = parent;
        }
        self.version += 1;
    }

    /// Uses a control in the header of structure `node`.
    pub(super) fn apply(&mut self, control: Control, node: NodeId, body: &Body) {
        let count = rv_layout::case_frames(body, node).len().max(1);
        let index = self.state.case_index(body, node);
        match control {
            Control::Toggle => {
                let shown = self.state.is_expanded(&self.defaults, node);
                self.state.expanded.insert(node, !shown);
            }
            Control::PrevCase => {
                self.state
                    .case_index
                    .insert(node, (index + count - 1) % count);
            }
            Control::NextCase => {
                self.state.case_index.insert(node, (index + 1) % count);
            }
            Control::CaseMode => {
                let all = self.state.shows_all_cases(body, node);
                self.state.all_cases.insert(node, !all);
            }
        }
        self.version += 1;
    }

    /// Switches the expression level of structure `target` on or off.
    pub(super) fn toggle_expressions(&mut self, target: NodeId) {
        if !self.state.expressions.remove(&target) {
            self.state.expressions.insert(target);
        }
        self.version += 1;
    }

    pub(super) fn expand_all(&mut self, body: &Body) {
        for s in structures(body) {
            self.state.expanded.insert(s, true);
        }
        self.version += 1;
    }

    /// Collapses everything but the function itself.
    pub(super) fn collapse_all(&mut self, body: &Body) {
        for s in structures(body).filter(|&s| s != body.root) {
            self.state.expanded.insert(s, false);
        }
        self.version += 1;
    }

    /// Back to the default expansion: from the outside in until about
    /// [`BUDGET`] items are visible.
    pub(super) fn auto_expand(&mut self, body: &Body) {
        self.state.expanded.clear();
        self.defaults = rv_layout::default_expanded(body, &self.state, BUDGET);
        self.version += 1;
    }

    pub(super) fn zoom_out(&mut self) {
        self.zoom = (self.zoom / ZOOM_STEP).max(MIN_ZOOM);
    }

    pub(super) fn zoom_in(&mut self) {
        self.zoom = (self.zoom * ZOOM_STEP).min(MAX_ZOOM);
    }

    pub(super) fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::splat(MARGIN);
    }

    /// Shows a whole diagram of size `diagram` in a canvas of size `canvas`
    /// (never above 100 %).
    pub(super) fn fit(&mut self, diagram: Vec2, canvas: Vec2) {
        let target = canvas - Vec2::splat(2.0 * MARGIN);
        self.zoom = (target.x / diagram.x)
            .min(target.y / diagram.y)
            .clamp(MIN_ZOOM, 1.0);
        self.pan = Vec2::splat(MARGIN);
    }

    /// Zooms by `factor`, keeping the diagram point under `anchor` (relative
    /// to the canvas corner) where it is.
    pub(super) fn zoom_about(&mut self, anchor: Vec2, factor: f32) {
        let world = (anchor - self.pan) / self.zoom;
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = anchor - world * self.zoom;
    }

    /// Scrolls the diagram point `center` to the middle of a canvas of size
    /// `canvas`, without leaving space before the diagram's start.
    pub(super) fn center_on(&mut self, center: Pos2, canvas: Vec2) {
        let center = center.to_vec2() * self.zoom;
        self.pan = canvas / 2.0 - center;
        self.pan.y = self.pan.y.min(MARGIN);
        self.pan.x = self.pan.x.min(MARGIN);
    }
}

/// All structures of a body, the function itself included.
fn structures(body: &Body) -> impl Iterator<Item = NodeId> + '_ {
    body.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Structure { .. }))
        .map(|(i, _)| NodeId(i as u32))
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    const SRC: &str = r#"
fn f(n: u32) -> u32 {
    let mut total = 0;
    for i in 0..n {
        match i % 3 {
            0 => { total += 1; }
            1 => { total += 2; }
            _ => { if i > 10 { total += i; } }
        }
    }
    total
}
"#;

    fn body() -> Body {
        rv_syn::load_str(SRC).functions[0].body.clone().unwrap()
    }

    fn find(body: &Body, label: &str) -> NodeId {
        let i = body
            .nodes
            .iter()
            .position(|n| n.label.starts_with(label))
            .unwrap();
        NodeId(i as u32)
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let mut view = FnView::new(&body(), false);
        view.pan = vec2(-30.0, 12.0);
        let anchor = vec2(200.0, 100.0);
        let world = (anchor - view.pan) / view.zoom;
        view.zoom_about(anchor, 1.5);
        assert_eq!(view.zoom, 1.5);
        let after = (anchor - view.pan) / view.zoom;
        assert!((after - world).length() < 1e-3);
        // The zoom stays within its limits.
        view.zoom_about(anchor, 1000.0);
        assert_eq!(view.zoom, MAX_ZOOM);
    }

    #[test]
    fn fit_shrinks_but_never_enlarges() {
        let mut view = FnView::new(&body(), false);
        view.fit(vec2(2000.0, 500.0), vec2(1000.0 + 2.0 * MARGIN, 800.0));
        assert_eq!(view.zoom, 0.5);
        assert_eq!(view.pan, Vec2::splat(MARGIN));
        view.fit(vec2(100.0, 50.0), vec2(1000.0, 800.0));
        assert_eq!(view.zoom, 1.0);
    }

    #[test]
    fn centering_leaves_no_space_before_the_start() {
        let mut view = FnView::new(&body(), false);
        view.center_on(pos2(1000.0, 10.0), vec2(600.0, 400.0));
        assert_eq!(view.pan, vec2(-700.0, MARGIN));
    }

    #[test]
    fn controls_change_the_state_and_ask_for_a_new_layout() {
        let body = body();
        let m = find(&body, "match");
        let mut view = FnView::new(&body, false);
        assert!(view.state.is_expanded(&view.defaults, m));
        view.apply(Control::Toggle, m, &body);
        assert!(!view.state.is_expanded(&view.defaults, m));
        view.apply(Control::PrevCase, m, &body);
        assert_eq!(view.state.case_index(&body, m), 2, "wraps around");
        view.apply(Control::NextCase, m, &body);
        assert_eq!(view.state.case_index(&body, m), 0);
        let all = view.state.shows_all_cases(&body, m);
        view.apply(Control::CaseMode, m, &body);
        assert_eq!(view.state.shows_all_cases(&body, m), !all);
        assert_eq!(view.version, 4);
    }

    #[test]
    fn collapse_all_keeps_the_function_open_and_reveal_opens_the_way() {
        let body = body();
        let mut view = FnView::new(&body, false);
        view.collapse_all(&body);
        assert!(view.state.is_expanded(&view.defaults, body.root));
        let lp = find(&body, "for");
        let m = find(&body, "match");
        assert!(!view.state.is_expanded(&view.defaults, lp));
        // Reveal the statement in the last case of the match.
        let inner = find(&body, "if");
        view.reveal(&body, inner);
        assert!(view.state.is_expanded(&view.defaults, lp));
        assert!(view.state.is_expanded(&view.defaults, m));
        assert_eq!(view.state.case_index(&body, m), 2);
        view.expand_all(&body);
        assert!(view.state.is_expanded(&view.defaults, inner));
    }

    #[test]
    fn expressions_toggle() {
        let body = body();
        let mut view = FnView::new(&body, true);
        assert!(view.state.expressions.contains(&body.root));
        view.toggle_expressions(body.root);
        assert!(view.state.expressions.is_empty());
        view.toggle_expressions(body.root);
        assert!(view.state.expressions.contains(&body.root));
    }
}
