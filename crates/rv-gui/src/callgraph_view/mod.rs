//! Call graph on module level (M5): modules as boxes, calls between them as
//! arrows, laid out in layers (callers left, callees right). Double-clicking
//! a module drills down to its functions with their callers and callees.

use egui::{RichText, Ui};
use rv_ir::{FnId, Project};

use crate::canvas::PanZoom;
use crate::widgets::export_button;

mod focus;
mod model;
mod overview;
mod svg;

pub(crate) use model::ModuleLayout;
pub(crate) use svg::module_svg;

use focus::{FocusCtx, FocusEvent};
use model::{Data, Focus};

/// The depth choices of the toolbar; the last one means "do not group".
const DEPTHS: std::ops::RangeInclusive<usize> = 1..=4;
const DEFAULT_DEPTH: usize = 2;

pub enum CallGraphAction {
    None,
    ShowFn(FnId),
    /// Status message (e.g. after an export).
    Message(String),
}

#[derive(Default)]
pub struct CallGraphView {
    data: Option<(u64, Data)>,
    crate_index: Option<usize>,
    /// One of [`DEPTHS`]; 0 until the first frame.
    depth: usize,
    graph: Option<(GraphKey, ModuleLayout)>,
    /// Drill-down: node of the current graph.
    focus: Option<usize>,
    /// The calls of the module in `focus`, computed when it is first shown.
    focus_calls: Option<Focus>,
    selected: Option<usize>,
    view: PanZoom,
    fit: bool,
    /// Module to drill into once the graph exists (`--callgraph ec_task`).
    open_module: Option<String>,
}

/// What a module graph was computed for.
#[derive(Clone, PartialEq)]
struct GraphKey {
    generation: u64,
    crate_name: String,
    depth: usize,
}

/// What the user did in the toolbar.
struct ToolbarEvents {
    /// Another crate or depth was chosen.
    changed: bool,
    export: bool,
}

impl CallGraphView {
    pub fn opening(module: Option<&str>) -> Self {
        CallGraphView {
            open_module: module.filter(|m| !m.is_empty()).map(str::to_string),
            ..CallGraphView::default()
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> CallGraphAction {
        if self.data.as_ref().is_none_or(|(g, _)| *g != generation) {
            self.data = Some((generation, Data::new(project)));
        }
        if self.depth == 0 {
            self.depth = DEFAULT_DEPTH;
        }
        let data = &self.data.as_ref().expect("computed above").1;
        if data.crates.is_empty() {
            ui.weak("No modules.");
            return CallGraphAction::None;
        }

        let toolbar = self.toolbar(ui);
        if toolbar.changed {
            self.focus = None;
            self.selected = None;
            self.fit = true;
        }
        self.update_graph(project, generation);
        self.open_wanted_module();
        ui.separator();
        let (key, layout) = self.graph.as_ref().expect("computed above");
        if toolbar.export {
            let name = format!("{}_modules", crate::export::file_name(&key.crate_name));
            if let Some(msg) = crate::export::export(&name, || module_svg(layout)) {
                return CallGraphAction::Message(msg);
            }
        }
        match self.focus {
            Some(node) => self.focus_ui(ui, project, node),
            None => {
                self.overview_ui(ui, project);
                CallGraphAction::None
            }
        }
    }

    /// Index of the crate shown, into the crates of `data`.
    fn current_crate(&self, data: &Data) -> usize {
        self.crate_index
            .unwrap_or(data.default_crate)
            .min(data.crates.len() - 1)
    }

    fn toolbar(&mut self, ui: &mut Ui) -> ToolbarEvents {
        let data = &self.data.as_ref().expect("computed by caller").1;
        let crate_index = self.current_crate(data);
        let mut events = ToolbarEvents {
            changed: false,
            export: false,
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("crate:");
            let chosen = crate_combo(ui, &data.crates, crate_index);
            if chosen != crate_index {
                self.crate_index = Some(chosen);
                events.changed = true;
            }
            ui.label("group modules to depth:");
            for d in DEPTHS {
                if ui
                    .selectable_label(self.depth == d, depth_label(d))
                    .clicked()
                    && self.depth != d
                {
                    self.depth = d;
                    events.changed = true;
                }
            }
            if self.focus.is_none() && ui.button("Fit").clicked() {
                self.fit = true;
            }
            events.export =
                self.focus.is_none() && export_button(ui, "Save the module graph as SVG or PNG");
            ui.weak("· double click a module: its functions · drag: move · Ctrl+wheel: zoom");
        });
        events
    }

    /// Computes the module graph when the project, crate or depth changed.
    fn update_graph(&mut self, project: &Project, generation: u64) {
        let data = &self.data.as_ref().expect("computed by caller").1;
        let key = GraphKey {
            generation,
            crate_name: data.crates[self.current_crate(data)].clone(),
            depth: if self.depth >= *DEPTHS.end() {
                usize::MAX
            } else {
                self.depth
            },
        };
        if self.graph.as_ref().is_none_or(|(k, ..)| *k != key) {
            let layout = ModuleLayout::new(
                project,
                &data.calls,
                &data.facts,
                &key.crate_name,
                key.depth,
            );
            // Node indices belong to the old graph: find the same modules
            // in the new one (they may be gone after a reload).
            self.focus = self.same_node(self.focus, &layout);
            self.selected = self.same_node(self.selected, &layout);
            self.graph = Some((key, layout));
            self.focus_calls = None;
            self.fit = true;
        }
    }

    /// The node of `new` with the same module path as `node` of the
    /// current graph.
    fn same_node(&self, node: Option<usize>, new: &ModuleLayout) -> Option<usize> {
        let (_, old) = self.graph.as_ref()?;
        let name = &old.graph.nodes.get(node?)?.key;
        new.graph.nodes.iter().position(|n| n.key == *name)
    }

    /// Drills into the module wanted on the command line, once.
    fn open_wanted_module(&mut self) {
        if let Some(wanted) = self.open_module.take()
            && let Some((_, layout)) = &self.graph
        {
            let suffix = format!("::{wanted}");
            self.focus = layout
                .graph
                .nodes
                .iter()
                .position(|n| n.key == wanted || n.key.ends_with(&suffix));
        }
    }

    /// Drill-down: callers | functions | callees of one module.
    fn focus_ui(&mut self, ui: &mut Ui, project: &Project, node: usize) -> CallGraphAction {
        let (key, layout) = self.graph.as_ref().expect("computed by caller");
        let ctx = FocusCtx {
            project,
            crate_name: &key.crate_name,
            graph: &layout.graph,
        };
        if self.focus_calls.as_ref().is_none_or(|f| f.node != node) {
            self.focus_calls = Some(Focus::new(project, &key.crate_name, &layout.graph, node));
        }
        let calls = self.focus_calls.as_ref().expect("computed above");
        match focus::focus_ui(ui, &ctx, calls) {
            Some(FocusEvent::ShowModule(module)) => {
                self.focus = module;
                self.selected = module;
                CallGraphAction::None
            }
            Some(FocusEvent::ShowFn(f)) => CallGraphAction::ShowFn(f),
            None => CallGraphAction::None,
        }
    }
}

/// Choice of the crate; returns the index of the chosen one.
fn crate_combo(ui: &mut Ui, crates: &[String], current: usize) -> usize {
    let mut chosen = current;
    egui::ComboBox::from_id_salt("cg_crate")
        .selected_text(RichText::new(&crates[chosen]).monospace())
        .show_ui(ui, |ui| {
            for (i, c) in crates.iter().enumerate() {
                ui.selectable_value(&mut chosen, i, RichText::new(c).monospace());
            }
        });
    chosen
}

fn depth_label(depth: usize) -> String {
    if depth == *DEPTHS.end() {
        "all".to_string()
    } else {
        depth.to_string()
    }
}
