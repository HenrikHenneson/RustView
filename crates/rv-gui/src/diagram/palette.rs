//! Colors and sizes of the function diagram, shared by the screen painter
//! and the SVG export. Sizes are in diagram units at zoom 1.

use egui::Color32;
use rv_ir::{Body, ExprKind, NodeKind, StmtKind, StructKind};

use crate::wires::{self, WireKind};

/// Font size of labels at zoom 1.
pub(super) const FONT_SIZE: f32 = 12.0;
/// Longer labels are cut.
pub(super) const MAX_LABEL_CHARS: usize = 60;
/// Corner radius of structures, frames and statements.
pub(super) const ITEM_RADIUS: f32 = 4.0;
/// Corner radius of expression boxes.
pub(super) const EXPR_RADIUS: f32 = 3.0;
/// Line width of thin outlines (frames, statements, expression boxes).
pub(super) const OUTLINE: f32 = 1.0;
/// How much of a structure's color its header gets.
pub(super) const HEADER_TINT: f32 = 0.35;
/// Line width of the outline that marks a parallel lane.
pub(super) const LANE_OUTLINE: f32 = 3.0;
/// Width of the bar that marks a statement starting a lane.
pub(super) const LANE_BAR: f32 = 4.0;
/// Line width of the wires between operands and operations.
pub(super) const EXPR_WIRE: f32 = 1.2;
/// Where "(empty)" goes in a frame without items, from its corner.
pub(super) const EMPTY_HINT_INSET: f32 = 8.0;
/// Height of a frame's title line.
pub(super) const FRAME_TITLE_HEIGHT: f32 = 22.0;

/// Colors of parallel lanes.
pub(super) const LANE_COLORS: [Color32; 6] = [
    Color32::from_rgb(0, 160, 120),
    Color32::from_rgb(210, 100, 0),
    Color32::from_rgb(40, 110, 220),
    Color32::from_rgb(200, 50, 120),
    Color32::from_rgb(120, 140, 0),
    Color32::from_rgb(110, 80, 200),
];

const LOOP_COLOR: Color32 = Color32::from_rgb(70, 130, 220);
const CASE_COLOR: Color32 = Color32::from_rgb(220, 160, 40);
const CLOSURE_COLOR: Color32 = Color32::from_rgb(150, 100, 210);
const CALL_COLOR: Color32 = Color32::from_rgb(80, 140, 210);
const LET_COLOR: Color32 = Color32::from_rgb(90, 170, 100);
const EXIT_COLOR: Color32 = Color32::from_rgb(220, 70, 60);
const NEUTRAL_COLOR: Color32 = Color32::from_rgb(140, 140, 140);

/// Border width of a structure: loops stand out.
pub(super) fn structure_border(kind: &StructKind) -> f32 {
    if kind.is_loop() { 3.0 } else { 1.5 }
}

/// Colors for the current theme.
pub(super) struct Palette {
    pub(super) dark: bool,
    pub(super) text: Color32,
    pub(super) weak: Color32,
    pub(super) exit: Color32,
    pub(super) hover: Color32,
    pub(super) selected: Color32,
    pub(super) frame_line: Color32,
    pub(super) case_title: Color32,
    pub(super) bg: Color32,
}

impl Palette {
    pub(super) fn new(v: &egui::Visuals) -> Self {
        Palette {
            dark: v.dark_mode,
            text: v.text_color(),
            weak: v.weak_text_color(),
            exit: if v.dark_mode {
                Color32::from_rgb(255, 120, 110)
            } else {
                Color32::from_rgb(200, 40, 30)
            },
            hover: v.widgets.hovered.fg_stroke.color,
            selected: v.selection.stroke.color,
            frame_line: v.widgets.noninteractive.bg_stroke.color,
            case_title: if v.dark_mode {
                Color32::from_rgb(240, 190, 90)
            } else {
                Color32::from_rgb(150, 95, 0)
            },
            bg: v.extreme_bg_color,
        }
    }

    pub(super) fn structure(&self, kind: &StructKind) -> Color32 {
        match kind {
            StructKind::Function => Color32::from_rgb(130, 130, 130),
            StructKind::Loop { .. } | StructKind::While { .. } | StructKind::For { .. } => {
                LOOP_COLOR
            }
            StructKind::If | StructKind::Match => CASE_COLOR,
            StructKind::Block { .. } | StructKind::LetElse => Color32::from_rgb(120, 135, 150),
            StructKind::Unsafe => Color32::from_rgb(210, 70, 70),
            StructKind::Closure { .. } => CLOSURE_COLOR,
            StructKind::Async { .. } => Color32::from_rgb(40, 170, 160),
        }
    }

    /// Nested frames alternate slightly so the depth stays readable.
    pub(super) fn frame_bg(&self, depth: u32) -> Color32 {
        let step = if depth.is_multiple_of(2) { 0 } else { 10 };
        let [r, g, b, _] = self.bg.to_array();
        if self.dark {
            Color32::from_rgb(
                r.saturating_add(14 + step),
                g.saturating_add(14 + step),
                b.saturating_add(16 + step),
            )
        } else {
            Color32::from_rgb(
                r.saturating_sub(6 + step),
                g.saturating_sub(6 + step),
                b.saturating_sub(4 + step),
            )
        }
    }

    fn stmt_base(kind: &StmtKind) -> Color32 {
        match kind {
            StmtKind::Let => LET_COLOR,
            StmtKind::Assign => Color32::from_rgb(200, 170, 60),
            StmtKind::Call | StmtKind::MethodCall => CALL_COLOR,
            StmtKind::Macro { .. } => NEUTRAL_COLOR,
            StmtKind::Item => Color32::from_rgb(110, 110, 110),
            StmtKind::Exit(_) => EXIT_COLOR,
            StmtKind::Expr => Color32::from_rgb(150, 150, 170),
        }
    }

    pub(super) fn stmt(&self, kind: &StmtKind) -> Color32 {
        let base = Self::stmt_base(kind);
        let mix = if self.dark { 0.28 } else { 0.18 };
        lerp(self.bg, base, mix)
    }

    pub(super) fn stmt_line(&self, kind: &StmtKind) -> Color32 {
        lerp(self.bg, Self::stmt_base(kind), 0.7)
    }

    /// Color of the info text in a structure header: exits stand out.
    pub(super) fn info(&self, text: &str) -> Color32 {
        if text.contains("exits") {
            self.exit
        } else {
            self.weak
        }
    }

    /// Color of notes without a lane: blue like shift registers.
    pub(super) fn note(&self) -> Color32 {
        wires::color("", WireKind::LoopBack, self.dark)
    }

    /// Fill of an expression box with outline color `base`.
    pub(super) fn expr_fill(&self, base: Color32) -> Color32 {
        lerp(self.bg, base, if self.dark { 0.35 } else { 0.22 })
    }
}

/// Color of an expression node by kind (variables: their wire color).
pub(super) fn expr_color(kind: ExprKind, text: &str, dark: bool, body: &Body) -> Color32 {
    use ExprKind as K;
    match kind {
        K::Var => wires::color(text, WireKind::Data, dark),
        K::Literal | K::Other => NEUTRAL_COLOR,
        K::Call | K::Method | K::Macro => CALL_COLOR,
        K::Operator | K::Field | K::Index | K::Cast | K::Ref | K::Try | K::Await => {
            Color32::from_rgb(200, 150, 40)
        }
        K::Construct | K::Closure => CLOSURE_COLOR,
        K::Bind => LET_COLOR,
        K::Assign => Color32::from_rgb(225, 120, 40),
        K::Return => EXIT_COLOR,
        K::Structure(id) => match &body.node(id).kind {
            NodeKind::Structure { kind, .. } if kind.is_loop() => LOOP_COLOR,
            NodeKind::Structure { kind, .. } if kind.is_case() => CASE_COLOR,
            _ => CLOSURE_COLOR,
        },
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
