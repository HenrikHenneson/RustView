//! Left panel: packages, targets, modules and functions.

use std::collections::{BTreeMap, HashSet};

use egui::{RichText, Ui};
use rv_ir::{FileId, FnId, FnOwner, ModuleId, Project, TestSummary};

#[derive(Default)]
pub struct TreeView {
    pub filter: String,
    /// Modules to force open until the selected function has been shown.
    reveal: HashSet<ModuleId>,
    scroll_to_selected: bool,
}

pub enum TreeAction {
    None,
    Select(FnId),
    /// Double click: jump to the function in the external editor.
    Open(FnId),
    OpenFile(FileId),
}

/// Functions with at least this many lines get their size shown.
const SHOW_SIZE_FROM: u32 = 50;
const MAX_SEARCH_HITS: usize = 500;

impl TreeView {
    /// Expand the tree down to `f` on the next frame.
    pub fn reveal(&mut self, ctx: &egui::Context, project: &Project, f: FnId) {
        self.reveal.clear();
        let mut m = Some(project.function(f).module);
        while let Some(id) = m {
            self.reveal.insert(id);
            m = project.module(id).parent;
        }
        self.scroll_to_selected = true;
        // The tree has already been drawn this frame.
        ctx.request_repaint();
    }

    pub fn ui(&mut self, ui: &mut Ui, project: &Project, selected: Option<FnId>) -> TreeAction {
        let mut action = TreeAction::None;
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Search functions…")
                .desired_width(f32::INFINITY),
        );
        ui.separator();
        // Horizontal scrolling instead of widening the panel for long names.
        egui::ScrollArea::both()
            .id_salt("tree_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                if self.filter.trim().is_empty() {
                    for pkg in &project.packages {
                        self.package(ui, project, pkg, selected, &mut action);
                    }
                } else {
                    self.search_results(ui, project, selected, &mut action);
                }
            });
        action
    }

    fn package(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        pkg: &rv_ir::Package,
        selected: Option<FnId>,
        action: &mut TreeAction,
    ) {
        let mut title = RichText::new(format!("📦 {}", pkg.name)).strong();
        if !pkg.in_workspace {
            title = title.italics();
        }
        let reveal = pkg
            .targets
            .iter()
            .any(|t| t.root_module.is_some_and(|m| self.reveal.contains(&m)));
        let header = egui::CollapsingHeader::new(title)
            .id_salt(("pkg", &pkg.name))
            .default_open(project.packages.len() <= 3)
            .open(reveal.then_some(true));
        let resp = header.show(ui, |ui| {
            if !pkg.in_workspace {
                ui.weak("not a member of the workspace");
            }
            if !pkg.tests.is_empty() {
                ui.weak(tests_label(&pkg.tests));
            }
            for target in &pkg.targets {
                let Some(root) = target.root_module else {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{} {}: could not load", target.kind.label(), target.name),
                    );
                    continue;
                };
                let title = format!("{} {}", target.kind.label(), target.name);
                egui::CollapsingHeader::new(RichText::new(title).strong())
                    .id_salt(("target", &pkg.name, &target.name, target.kind))
                    .default_open(pkg.targets.len() <= 2)
                    .open(self.reveal.contains(&root).then_some(true))
                    .show(ui, |ui| {
                        self.module_contents(ui, project, root, selected, action)
                    });
            }
            if !pkg.unlinked_files.is_empty() {
                egui::CollapsingHeader::new(
                    RichText::new(format!(
                        "not in any module tree ({})",
                        pkg.unlinked_files.len()
                    ))
                    .weak(),
                )
                .id_salt(("unlinked", &pkg.name))
                .show(ui, |ui| {
                    for &f in &pkg.unlinked_files {
                        let path = &project.file(f).path;
                        let shown = path.strip_prefix(&pkg.root_dir).unwrap_or(path);
                        if ui
                            .selectable_label(false, shown.display().to_string())
                            .clicked()
                        {
                            *action = TreeAction::OpenFile(f);
                        }
                    }
                });
            }
        });
        resp.header_response
            .on_hover_text(pkg.manifest_path.display().to_string());
    }

    fn module_contents(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        m: ModuleId,
        selected: Option<FnId>,
        action: &mut TreeAction,
    ) {
        let module = project.module(m);
        for &child in &module.children {
            let c = project.module(child);
            let file = project.file(c.file);
            let mut title = RichText::new(format!("mod {}", c.name));
            if file.parse_error.is_some() {
                title = title.color(ui.visuals().error_fg_color);
            }
            egui::CollapsingHeader::new(title)
                .id_salt(("mod", c.qual_name(), m))
                .open(self.reveal.contains(&child).then_some(true))
                .show(ui, |ui| {
                    self.module_contents(ui, project, child, selected, action)
                });
        }

        // Free functions first, then one group per impl / trait block.
        let mut groups: BTreeMap<String, Vec<FnId>> = BTreeMap::new();
        for &f in &module.functions {
            let func = project.function(f);
            match func.owner.group_label() {
                None if matches!(func.owner, FnOwner::Nested { .. }) => {}
                None => self.function_row(ui, project, f, selected, action),
                Some(label) => groups.entry(label).or_default().push(f),
            }
        }
        for (label, fns) in groups {
            let contains_selected = selected.is_some_and(|s| fns.contains(&s));
            let reveal = contains_selected && self.reveal.contains(&m);
            egui::CollapsingHeader::new(RichText::new(label.clone()).monospace())
                .id_salt(("group", module.qual_name(), &label))
                .open(reveal.then_some(true))
                .show(ui, |ui| {
                    for f in fns {
                        self.function_row(ui, project, f, selected, action);
                    }
                });
        }
    }

    fn function_row(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        f: FnId,
        selected: Option<FnId>,
        action: &mut TreeAction,
    ) {
        let func = project.function(f);
        let is_selected = selected == Some(f);
        ui.horizontal(|ui| {
            let resp = ui.selectable_label(is_selected, fn_label(func));
            let resp = resp.on_hover_text(&func.sig.text);
            if resp.clicked() {
                *action = TreeAction::Select(f);
            }
            if resp.double_clicked() {
                *action = TreeAction::Open(f);
            }
            if is_selected && self.scroll_to_selected {
                resp.scroll_to_me(Some(egui::Align::Center));
                self.scroll_to_selected = false;
                // Kept until now: each header level opens with an animation
                // and only shows its body from the following frame on.
                self.reveal.clear();
            }
            let lines = func.span.line_count();
            if lines >= SHOW_SIZE_FROM {
                ui.weak(format!("{lines} lines"));
            }
        });
        // Functions declared inside this one.
        for &nested in &project.module(func.module).functions {
            if matches!(project.function(nested).owner, FnOwner::Nested { parent } if parent == f) {
                ui.indent(("nested", f), |ui| {
                    self.function_row(ui, project, nested, selected, action)
                });
            }
        }
    }

    fn search_results(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        selected: Option<FnId>,
        action: &mut TreeAction,
    ) {
        let needle = self.filter.trim().to_lowercase();
        let mut hits = 0;
        for (i, func) in project.functions.iter().enumerate() {
            if !func.qual_path.to_lowercase().contains(&needle) {
                continue;
            }
            hits += 1;
            if hits > MAX_SEARCH_HITS {
                continue;
            }
            let f = FnId(i as u32);
            let resp = ui
                .selectable_label(
                    selected == Some(f),
                    RichText::new(&func.qual_path).monospace(),
                )
                .on_hover_text(&func.sig.text);
            if resp.clicked() {
                *action = TreeAction::Select(f);
            }
            if resp.double_clicked() {
                *action = TreeAction::Open(f);
            }
        }
        if hits == 0 {
            ui.weak("no matches");
        } else if hits > MAX_SEARCH_HITS {
            ui.weak(format!("… {} more", hits - MAX_SEARCH_HITS));
        }
    }
}

fn fn_label(func: &rv_ir::Function) -> RichText {
    let mut s = String::new();
    if func.sig.is_const {
        s.push_str("const ");
    }
    if func.sig.is_async {
        s.push_str("async ");
    }
    if func.sig.is_unsafe {
        s.push_str("unsafe ");
    }
    s.push_str("fn ");
    s.push_str(&func.name);
    RichText::new(s).monospace()
}

fn tests_label(t: &TestSummary) -> String {
    let mut parts = Vec::new();
    let mut add = |n: u32, what: &str| {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    };
    add(t.test_fns, "#[test] fns");
    add(t.cfg_test_modules, "cfg(test) modules");
    add(t.test_targets, "test targets");
    add(t.bench_targets, "benches");
    add(t.example_targets, "examples");
    format!("tests (not shown): {}", parts.join(" · "))
}
