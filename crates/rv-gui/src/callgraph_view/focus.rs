//! Drill-down into one module: callers | its functions | callees.

use egui::{Color32, RichText, Ui};
use rv_analysis::ModuleGraph;
use rv_ir::{FnId, Project};

use super::model::{FnRow, Focus, ModuleCalls, fn_label};
use crate::style::{CALLS_IN, CALLS_OUT};
use crate::widgets::mono_link;

/// Function pairs listed per module on the other side.
const MAX_PAIRS: usize = 40;

/// What the user asked for in the drill-down.
pub(super) enum FocusEvent {
    /// Show this module instead (`None`: back to the graph of all modules).
    ShowModule(Option<usize>),
    ShowFn(FnId),
}

/// What all parts of the drill-down need.
pub(super) struct FocusCtx<'a> {
    pub project: &'a Project,
    pub crate_name: &'a str,
    pub graph: &'a ModuleGraph,
}

/// One of the outer columns: the calls into or out of the module.
struct Side {
    title: &'static str,
    color: Color32,
    /// Calls into the module (the outside function is the caller).
    incoming: bool,
    /// Id of the column's scroll area and indents.
    salt: &'static str,
}

const CALLERS: Side = Side {
    title: "called from",
    color: CALLS_IN,
    incoming: true,
    salt: "cg_in",
};
const CALLEES: Side = Side {
    title: "calls into",
    color: CALLS_OUT,
    incoming: false,
    salt: "cg_out",
};

pub(super) fn focus_ui(ui: &mut Ui, ctx: &FocusCtx, focus: &Focus) -> Option<FocusEvent> {
    let n = &ctx.graph.nodes[focus.node];
    let mut event = None;
    ui.horizontal(|ui| {
        if ui.link("all modules").clicked() {
            event = Some(FocusEvent::ShowModule(None));
        }
        ui.label("›");
        ui.strong(RichText::new(&n.key).monospace());
        ui.weak(format!(
            "{} functions · {} lines",
            n.functions.len(),
            n.lines
        ));
    });
    ui.separator();
    ui.columns(3, |cols| {
        let callers = side_ui(&mut cols[0], ctx, &CALLERS, &focus.incoming);
        let function = functions_ui(&mut cols[1], ctx, &n.key, &focus.functions);
        let callees = side_ui(&mut cols[2], ctx, &CALLEES, &focus.outgoing);
        event = function.or(callees).or(callers).or(event.take());
    });
    event
}

/// Middle column: the module's functions with their calls from and to other modules.
fn functions_ui(
    ui: &mut Ui,
    ctx: &FocusCtx,
    module: &str,
    functions: &[FnRow],
) -> Option<FocusEvent> {
    let mut event = None;
    ui.label(RichText::new(format!("functions in {module}")).strong());
    egui::ScrollArea::vertical()
        .id_salt("cg_fns")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in functions {
                let func = ctx.project.function(row.f);
                ui.horizontal_wrapped(|ui| {
                    if mono_link(ui, &row.name)
                        .on_hover_text(&func.sig.text)
                        .clicked()
                    {
                        event = Some(FocusEvent::ShowFn(row.f));
                    }
                    if row.calls_in > 0 {
                        ui.label(RichText::new(format!("←{}", row.calls_in)).color(CALLS_IN));
                    }
                    if row.calls_out > 0 {
                        ui.label(RichText::new(format!("→{}", row.calls_out)).color(CALLS_OUT));
                    }
                    ui.weak(format!("{} lines", func.span.line_count()));
                });
            }
        });
    event
}

fn side_ui(ui: &mut Ui, ctx: &FocusCtx, side: &Side, calls: &[ModuleCalls]) -> Option<FocusEvent> {
    let mut event = None;
    ui.label(RichText::new(side.title).strong().color(side.color));
    egui::ScrollArea::vertical()
        .id_salt(side.salt)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if calls.is_empty() {
                ui.weak("(none)");
            }
            for module in calls {
                event = module_calls_ui(ui, ctx, side, module).or(event.take());
            }
        });
    event
}

/// The calls from or to one other module: its name and the function pairs.
fn module_calls_ui(
    ui: &mut Ui,
    ctx: &FocusCtx,
    side: &Side,
    calls: &ModuleCalls,
) -> Option<FocusEvent> {
    let mut event = None;
    let header = format!("{} ({})", ctx.graph.nodes[calls.other].key, calls.total);
    if mono_link(ui, header)
        .on_hover_text("show this module")
        .clicked()
    {
        event = Some(FocusEvent::ShowModule(Some(calls.other)));
    }
    ui.indent((side.salt, calls.other), |ui| {
        for &(pair, count) in calls.pairs.iter().take(MAX_PAIRS) {
            event = pair_row(ui, ctx, side, pair, count).or(event.take());
        }
        if calls.pairs.len() > MAX_PAIRS {
            ui.weak(format!("… {} more", calls.pairs.len() - MAX_PAIRS));
        }
    });
    event
}

/// One (caller, callee) pair: the function outside the module as a link, the
/// one inside, and how often the call occurs.
fn pair_row(
    ui: &mut Ui,
    ctx: &FocusCtx,
    side: &Side,
    (caller, callee): (FnId, FnId),
    count: usize,
) -> Option<FocusEvent> {
    let (outside, inside, arrow) = if side.incoming {
        (caller, callee, "→")
    } else {
        (callee, caller, "←")
    };
    let mut event = None;
    ui.horizontal_wrapped(|ui| {
        if mono_link(ui, fn_label(ctx.project, ctx.crate_name, outside)).clicked() {
            event = Some(FocusEvent::ShowFn(outside));
        }
        ui.weak(arrow);
        ui.weak(RichText::new(&ctx.project.function(inside).name).monospace());
        if count > 1 {
            ui.weak(format!("×{count}"));
        }
    });
    event
}
