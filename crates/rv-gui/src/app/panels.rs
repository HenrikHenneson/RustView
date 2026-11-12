//! The panels with the views: function tree, code, and the tabs in the
//! middle. Each draws its view and applies what the user did there.

use egui::Ui;

use super::App;
use crate::callgraph_view::CallGraphAction;
use crate::code_view::CodeAction;
use crate::diagram::DiagramAction;
use crate::report_view::ReportAction;
use crate::threads::ThreadsAction;
use crate::tree::TreeAction;
use crate::widgets::loading;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CenterTab {
    Diagram,
    Threads,
    CallGraph,
    Report,
}

impl App {
    /// Left panel: the function tree. Its action is applied after all
    /// panels are drawn, see [`Self::apply_tree_action`].
    pub(super) fn tree_panel(&mut self, ui: &mut Ui) -> TreeAction {
        let mut action = TreeAction::None;
        // Panels remember the width their content used, so the content has
        // to claim the full width or the panel shrinks to fit it.
        egui::Panel::left("tree_panel")
            .resizable(true)
            .default_size(340.0)
            .min_size(180.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                match &self.project {
                    Some(p) => action = self.tree.ui(ui, p, self.selected),
                    None => {
                        ui.spinner();
                    }
                }
            });
        action
    }

    /// Right panel: the source code. Its action is applied after all panels
    /// are drawn, see [`Self::apply_code_action`].
    pub(super) fn code_panel(&mut self, ui: &mut Ui) -> CodeAction {
        let mut action = CodeAction::None;
        egui::Panel::right("code_panel")
            .resizable(true)
            .default_size(760.0)
            .min_size(300.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(p) = &self.project {
                    action = self.code.ui(ui, p, &mut self.highlighter);
                }
            });
        action
    }

    pub(super) fn apply_tree_action(&mut self, action: &TreeAction) {
        match *action {
            TreeAction::None => {}
            TreeAction::Select(f) => self.select(f),
            TreeAction::Open(f) => {
                self.select(f);
                self.open_fn_in_editor(f);
            }
            TreeAction::OpenFile(file) => {
                if let Some(p) = &self.project {
                    self.code.show_file(p, file);
                }
            }
        }
    }

    pub(super) fn apply_code_action(&mut self, action: CodeAction) {
        match action {
            CodeAction::None => {}
            CodeAction::Clicked { line } => {
                // Show the innermost statement of the clicked line in the diagram.
                if let (Some(p), Some(f), Some(file)) =
                    (&self.project, self.selected, self.code.file)
                {
                    self.diagram.reveal_line(p, f, file, line);
                }
            }
            CodeAction::OpenInEditor { line, col } => {
                if let Some((p, file)) = self.project.as_ref().zip(self.code.file) {
                    let path = p.file(file).path.clone();
                    self.open_in_editor(path, line, col);
                }
            }
        }
    }

    /// Selections made outside the tree (e.g. after a reload) become visible in it.
    pub(super) fn reveal_selection(&mut self, ctx: &egui::Context, tree_action: &TreeAction) {
        if self.selected == self.revealed {
            return;
        }
        if let (Some(p), Some(f)) = (&self.project, self.selected)
            && !matches!(tree_action, TreeAction::Select(_) | TreeAction::Open(_))
        {
            self.tree.reveal(ctx, p, f);
        }
        self.revealed = self.selected;
    }

    /// The tabs in the middle and the view of the chosen one.
    pub(super) fn center_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.center, CenterTab::Diagram, "Function diagram");
            ui.selectable_value(&mut self.center, CenterTab::Threads, "Threads")
                .on_hover_text("All threads and tasks of the project");
            ui.selectable_value(&mut self.center, CenterTab::CallGraph, "Call graph")
                .on_hover_text("Calls between modules, with drill-down to functions");
            ui.selectable_value(&mut self.center, CenterTab::Report, "Report")
                .on_hover_text(
                    "Threads, communication, modules and hot spots as text, with links to the code",
                );
        });
        ui.separator();
        match self.center {
            CenterTab::Diagram => self.diagram_panel(ui),
            CenterTab::Threads => self.threads_panel(ui),
            CenterTab::CallGraph => self.callgraph_panel(ui),
            CenterTab::Report => self.report_panel(ui),
        }
    }

    fn report_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            loading(ui);
            return;
        };
        match self.report.ui(ui, project, self.generation) {
            ReportAction::None => {}
            ReportAction::Message(msg) => self.show_message(msg),
            ReportAction::ShowFn(f) => self.select(f),
            // The report tab stays open, the code panel and diagram follow.
            ReportAction::Goto(file, line) => self.goto_line(file, line),
        }
    }

    fn callgraph_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            loading(ui);
            return;
        };
        match self.callgraph.ui(ui, project, self.generation) {
            CallGraphAction::None => {}
            CallGraphAction::Message(msg) => self.show_message(msg),
            CallGraphAction::ShowFn(f) => {
                self.select(f);
                self.center = CenterTab::Diagram;
            }
        }
    }

    fn threads_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            loading(ui);
            return;
        };
        match self.threads.ui(ui, project, self.generation) {
            ThreadsAction::None => {}
            ThreadsAction::Message(msg) => self.show_message(msg),
            ThreadsAction::ShowFn(f) => {
                self.select(f);
                self.center = CenterTab::Diagram;
            }
            ThreadsAction::ShowNode(f, node) => {
                self.select(f);
                if let Some(project) = &self.project {
                    self.diagram.reveal_node(project, f, node);
                    if let Some(body) = &project.function(f).body {
                        let n = body.node(node);
                        self.code.show_span(project, n.span.clone(), n.head.clone());
                    }
                }
                self.center = CenterTab::Diagram;
            }
        }
    }

    fn diagram_panel(&mut self, ui: &mut Ui) {
        let Some(project) = &self.project else {
            loading(ui);
            return;
        };
        let Some(f) = self.selected else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Select a function on the left");
                ui.weak("Click: show diagram and source · Double click: open in editor");
            });
            return;
        };
        let action = self.diagram.ui(ui, project, self.generation, f);
        let Some(body) = &project.function(f).body else {
            return;
        };
        match action {
            DiagramAction::None => {}
            DiagramAction::Select(n) => {
                let node = body.node(n);
                self.code
                    .show_span(project, node.span.clone(), node.head.clone());
            }
            DiagramAction::Message(msg) => self.show_message(msg),
            DiagramAction::ShowSpan(span) => {
                self.code.show_span(project, span.clone(), span);
            }
            DiagramAction::Open(n) => {
                let head = body.node(n).head.clone();
                let path = project.file(head.file).path.clone();
                self.open_in_editor(path, head.start.line, head.start.col);
            }
        }
    }
}
