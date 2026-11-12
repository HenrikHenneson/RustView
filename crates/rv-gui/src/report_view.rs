//! The Markdown outputs (`--report`, `--maintainability`) as tabs:
//! formatted, with `file:line` locations and function names as links.

use egui::{RichText, Ui};
use rv_ir::{FileId, FnId, Project};

mod parse;

use parse::{Block, BlockKind, Seg, base_dir, parse};

pub enum ReportAction {
    None,
    /// A `file:line` location was clicked.
    Goto(FileId, u32),
    /// A function name was clicked.
    ShowFn(FnId),
    Message(String),
}

/// Which text a [`ReportView`] shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Threads, communication, modules and hot spots.
    Report,
    /// Score and the functions, files and module cycles over their targets.
    Maintainability,
}

impl ReportKind {
    /// Name in messages and file names, and the command line option.
    fn name(self) -> &'static str {
        match self {
            ReportKind::Report => "report",
            ReportKind::Maintainability => "maintainability",
        }
    }

    fn text(self, project: &Project) -> String {
        match self {
            ReportKind::Report => rv_analysis::report(project),
            ReportKind::Maintainability => {
                let calls = rv_analysis::CallGraph::new(project);
                let found = rv_analysis::Maintainability::new(project, &calls);
                rv_analysis::maintainability_report(project, &found)
            }
        }
    }
}

pub struct ReportView {
    kind: ReportKind,
    /// Project generation the report was made for.
    generation: Option<u64>,
    text: String,
    blocks: Vec<Block>,
    /// Generate on the next frame (after "generating…" was drawn once).
    pending: bool,
}

impl ReportView {
    pub fn new(kind: ReportKind) -> Self {
        ReportView {
            kind,
            generation: None,
            text: String::new(),
            blocks: Vec::new(),
            pending: false,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ReportAction {
        if self.generation != Some(generation) && !self.pending {
            self.pending = true;
            ui.ctx().request_repaint();
        } else if self.pending {
            let started = std::time::Instant::now();
            self.text = self.kind.text(project);
            self.blocks = parse(project, &self.text);
            self.generation = Some(generation);
            self.pending = false;
            log::info!("{} generated in {:?}", self.kind.name(), started.elapsed());
        }
        if self.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("generating the {}…", self.kind.name()));
            });
            return ReportAction::None;
        }

        let mut action = self.toolbar(ui, project);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(1100.0));
                for (i, block) in self.blocks.iter().enumerate() {
                    if let Some(a) = draw_block(ui, block, i) {
                        action = a;
                    }
                }
            });
        action
    }

    /// Save, copy and regenerate.
    fn toolbar(&mut self, ui: &mut Ui, project: &Project) -> ReportAction {
        let mut action = ReportAction::None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Save…")
                .on_hover_text("Save the text as Markdown")
                .clicked()
            {
                action = self.save(project);
            }
            if ui
                .button("Copy")
                .on_hover_text("Copy the Markdown text, e.g. into a chat with an AI assistant")
                .clicked()
            {
                ui.ctx().copy_text(self.text.clone());
                action =
                    ReportAction::Message(format!("{} copied to the clipboard", self.kind.name()));
            }
            if ui.button("Regenerate").clicked() {
                self.pending = true;
            }
            ui.weak(format!(
                "{} KB · also `rustview <project> --{}` · click a location to show the code",
                self.text.len().div_ceil(1024),
                self.kind.name()
            ));
        });
        action
    }

    fn save(&self, project: &Project) -> ReportAction {
        let name = base_dir(&project.root)
            .file_name()
            .map(|n| {
                format!(
                    "{}_{}.md",
                    crate::export::file_name(&n.to_string_lossy()),
                    self.kind.name()
                )
            })
            .unwrap_or_else(|| format!("{}.md", self.kind.name()));
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Save {}", self.kind.name()))
            .set_file_name(name)
            .add_filter("Markdown", &["md"])
            .save_file()
        else {
            return ReportAction::None;
        };
        ReportAction::Message(match std::fs::write(&path, &self.text) {
            Ok(()) => format!("saved {}", path.display()),
            Err(e) => format!("could not save {}: {e}", path.display()),
        })
    }
}

/// `index` is the position of the block in the text (for widget ids).
fn draw_block(ui: &mut Ui, block: &Block, index: usize) -> Option<ReportAction> {
    let mut action = None;
    let segs = &block.segs;
    match &block.kind {
        BlockKind::Table(rows) => action = draw_table(ui, rows, index),
        BlockKind::Gap => {
            ui.add_space(6.0);
        }
        BlockKind::Title => {
            ui.horizontal_wrapped(|ui| action = draw_segs(ui, segs, Some(22.0), true));
        }
        BlockKind::Heading => {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| action = draw_segs(ui, segs, Some(18.0), true));
            ui.separator();
        }
        BlockKind::Subheading => {
            ui.horizontal_wrapped(|ui| action = draw_segs(ui, segs, Some(15.0), true));
        }
        BlockKind::Item(level) => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0 + 18.0 * *level as f32);
                ui.label(if *level == 0 { "•  " } else { "◦  " });
                action = draw_segs(ui, segs, None, false);
            });
        }
        BlockKind::Paragraph => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                action = draw_segs(ui, segs, None, false);
            });
        }
    }
    action
}

fn draw_table(ui: &mut Ui, rows: &[Vec<Vec<Seg>>], index: usize) -> Option<ReportAction> {
    let mut action = None;
    egui::Grid::new(("report_table", index))
        .striped(true)
        .spacing([18.0, 4.0])
        .show(ui, |ui| {
            for (r, row) in rows.iter().enumerate() {
                for cell in row {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        action = draw_segs(ui, cell, None, r == 0).or(action.take());
                    });
                }
                ui.end_row();
            }
        });
    action
}

/// The segments of a block in a row; `size` and `strong` are those of
/// headings. Returns what a clicked link asks for.
fn draw_segs(ui: &mut Ui, segs: &[Seg], size: Option<f32>, strong: bool) -> Option<ReportAction> {
    let mut action = None;
    let sized = |t: RichText| match size {
        Some(s) => t.size(s),
        None => t,
    };
    for seg in segs {
        match seg {
            Seg::Text(t) => {
                let t = sized(RichText::new(t));
                ui.label(if strong { t.strong() } else { t });
            }
            Seg::Bold(t) => {
                ui.label(sized(RichText::new(t).strong()));
            }
            Seg::Code(t, None) => {
                ui.label(sized(RichText::new(t).code()));
            }
            Seg::Code(t, Some(f)) => {
                let link = ui.link(sized(RichText::new(t).code()));
                if link.on_hover_text("show the function").clicked() {
                    action = Some(ReportAction::ShowFn(*f));
                }
            }
            Seg::Loc(t, file, line) => {
                let link = ui.link(t.as_str());
                if link.on_hover_text("show in the code").clicked() {
                    action = Some(ReportAction::Goto(*file, *line));
                }
            }
        }
    }
    action
}
