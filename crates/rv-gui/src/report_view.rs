//! The Markdown project report (`--report`) as a tab: formatted, with
//! `file:line` locations and function names as links.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use rv_ir::{FileId, FnId, Project};

pub enum ReportAction {
    None,
    /// A `file:line` location was clicked.
    Goto(FileId, u32),
    /// A function name was clicked.
    ShowFn(FnId),
    Message(String),
}

#[derive(Default)]
pub struct ReportView {
    /// Project generation the report was made for.
    generation: Option<u64>,
    text: String,
    blocks: Vec<Block>,
    /// Generate on the next frame (after "generating…" was drawn once).
    pending: bool,
}

enum BlockKind {
    Title,
    Heading,
    Subheading,
    /// List item with indent level.
    Item(usize),
    Paragraph,
    Gap,
}

struct Block {
    kind: BlockKind,
    segs: Vec<Seg>,
}

enum Seg {
    Text(String),
    Bold(String),
    Code(String, Option<FnId>),
    Loc(String, FileId, u32),
}

impl ReportView {
    pub fn ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> ReportAction {
        let mut action = ReportAction::None;
        if self.generation != Some(generation) && !self.pending {
            self.pending = true;
            ui.ctx().request_repaint();
        } else if self.pending {
            let started = std::time::Instant::now();
            self.text = rv_analysis::report(project);
            self.blocks = parse(project, &self.text);
            self.generation = Some(generation);
            self.pending = false;
            log::info!("report generated in {:?}", started.elapsed());
        }
        if self.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("generating the report…");
            });
            return action;
        }

        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Save…")
                .on_hover_text("Save the report as Markdown")
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
                action = ReportAction::Message("report copied to the clipboard".into());
            }
            if ui.button("Regenerate").clicked() {
                self.pending = true;
            }
            ui.weak(format!(
                "{} KB · also `rustview <project> --report` · click a location to show the code",
                self.text.len().div_ceil(1024)
            ));
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(1100.0));
                for block in &self.blocks {
                    if let Some(a) = draw_block(ui, block) {
                        action = a;
                    }
                }
            });
        action
    }

    fn save(&self, project: &Project) -> ReportAction {
        let name = base_dir(&project.root)
            .file_name()
            .map(|n| {
                format!(
                    "{}_report.md",
                    crate::export::file_name(&n.to_string_lossy())
                )
            })
            .unwrap_or_else(|| "report.md".into());
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save report")
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

fn draw_block(ui: &mut Ui, block: &Block) -> Option<ReportAction> {
    let mut action = None;
    let mut segs = |ui: &mut Ui, size: Option<f32>, strong: bool| {
        for seg in &block.segs {
            let sized = |t: RichText| match size {
                Some(s) => t.size(s),
                None => t,
            };
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
                    if ui
                        .link(sized(RichText::new(t).code()))
                        .on_hover_text("show the function")
                        .clicked()
                    {
                        action = Some(ReportAction::ShowFn(*f));
                    }
                }
                Seg::Loc(t, file, line) => {
                    if ui
                        .link(t.as_str())
                        .on_hover_text("show in the code")
                        .clicked()
                    {
                        action = Some(ReportAction::Goto(*file, *line));
                    }
                }
            }
        }
    };
    match block.kind {
        BlockKind::Gap => {
            ui.add_space(6.0);
        }
        BlockKind::Title => {
            ui.horizontal_wrapped(|ui| segs(ui, Some(22.0), true));
        }
        BlockKind::Heading => {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| segs(ui, Some(18.0), true));
            ui.separator();
        }
        BlockKind::Subheading => {
            ui.horizontal_wrapped(|ui| segs(ui, Some(15.0), true));
        }
        BlockKind::Item(level) => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0 + 18.0 * level as f32);
                ui.label(if level == 0 { "•  " } else { "◦  " });
                segs(ui, None, false);
            });
        }
        BlockKind::Paragraph => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                segs(ui, None, false);
            });
        }
    }
    action
}

fn base_dir(root: &Path) -> PathBuf {
    if root.is_file() {
        root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        root.to_path_buf()
    }
}

/// Lookup tables for links.
struct Targets {
    /// Path relative to the project directory (with `/`) -> file.
    files: HashMap<String, FileId>,
    /// Qualified path and unique `Type::name` / `module::name` suffixes -> function.
    functions: HashMap<String, Option<FnId>>,
}

impl Targets {
    fn new(project: &Project) -> Self {
        let base = base_dir(&project.root);
        let files = project
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let rel = f.path.strip_prefix(&base).unwrap_or(&f.path);
                (rel.to_string_lossy().replace('\\', "/"), FileId(i as u32))
            })
            .collect();
        let mut functions: HashMap<String, Option<FnId>> = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            let id = FnId(i as u32);
            functions.insert(f.qual_path.clone(), Some(id));
            let parts: Vec<&str> = f.qual_path.rsplitn(3, "::").collect();
            if let [last, prev, _] = parts.as_slice() {
                // Ambiguous short names get no link.
                functions
                    .entry(format!("{prev}::{last}"))
                    .and_modify(|e| {
                        if *e != Some(id) {
                            *e = None
                        }
                    })
                    .or_insert(Some(id));
            }
        }
        // Full paths win over equal short names of other functions.
        for (i, f) in project.functions.iter().enumerate() {
            functions.insert(f.qual_path.clone(), Some(FnId(i as u32)));
        }
        Targets { files, functions }
    }
}

fn parse(project: &Project, text: &str) -> Vec<Block> {
    let targets = Targets::new(project);
    let mut blocks = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            blocks.push(Block {
                kind: BlockKind::Paragraph,
                segs: vec![Seg::Code(line.to_string(), None)],
            });
            continue;
        }
        let trimmed = line.trim_start();
        let indent = (line.len() - trimmed.len()) / 2;
        let (kind, rest) = if let Some(r) = line.strip_prefix("### ") {
            (BlockKind::Subheading, r)
        } else if let Some(r) = line.strip_prefix("## ") {
            (BlockKind::Heading, r)
        } else if let Some(r) = line.strip_prefix("# ") {
            (BlockKind::Title, r)
        } else if let Some(r) = trimmed.strip_prefix("- ") {
            (BlockKind::Item(indent), r)
        } else if trimmed.is_empty() {
            (BlockKind::Gap, "")
        } else {
            (BlockKind::Paragraph, trimmed)
        };
        blocks.push(Block {
            kind,
            segs: inline(rest, &targets),
        });
    }
    blocks
}

/// Splits a line into text, `**bold**`, `` `code` `` and `file.rs:12, 40` locations.
fn inline(text: &str, targets: &Targets) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let code = rest.find('`');
        let bold = rest.find("**");
        match (code, bold) {
            (Some(c), b) if b.is_none_or(|b| c < b) => {
                locations(&rest[..c], targets, &mut segs);
                let after = &rest[c + 1..];
                let end = after.find('`').unwrap_or(after.len());
                let code = &after[..end];
                let f = targets.functions.get(code).copied().flatten();
                segs.push(Seg::Code(code.to_string(), f));
                rest = after.get(end + 1..).unwrap_or("");
            }
            (_, Some(b)) => {
                locations(&rest[..b], targets, &mut segs);
                let after = &rest[b + 2..];
                let end = after.find("**").unwrap_or(after.len());
                let inner = &after[..end];
                // `**`code`**`: keep the code (and its link), shown as is.
                if inner.starts_with('`') {
                    segs.extend(inline(inner, targets));
                } else {
                    segs.push(Seg::Bold(inner.to_string()));
                }
                rest = after.get(end + 2..).unwrap_or("");
            }
            _ => {
                locations(rest, targets, &mut segs);
                rest = "";
            }
        }
    }
    segs
}

/// Plain text with `path.rs:12` and following `, 40, 41` line numbers as links.
fn locations(text: &str, targets: &Targets, segs: &mut Vec<Seg>) {
    let mut plain = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(".rs:") {
        let start = rest[..i]
            .rfind(|c: char| c.is_whitespace() || c == '(')
            .map_or(0, |s| s + 1);
        let digits = rest[i + 4..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len() - i - 4);
        let path = &rest[start..i + 3];
        let (Some(&file), Ok(line)) = (
            targets.files.get(path),
            rest[i + 4..i + 4 + digits].parse::<u32>(),
        ) else {
            plain.push_str(&rest[..i + 4]);
            rest = &rest[i + 4..];
            continue;
        };
        plain.push_str(&rest[..start]);
        push_text(segs, &mut plain);
        segs.push(Seg::Loc(
            rest[start..i + 4 + digits].to_string(),
            file,
            line,
        ));
        rest = &rest[i + 4 + digits..];
        // More lines of the same file: `(src/a.rs:12, 40, 41)`.
        while let Some(more) = rest.strip_prefix(", ") {
            let n = more
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(more.len());
            let Ok(line) = more[..n].parse::<u32>() else {
                break;
            };
            segs.push(Seg::Text(", ".into()));
            segs.push(Seg::Loc(more[..n].to_string(), file, line));
            rest = &more[n..];
        }
    }
    plain.push_str(rest);
    push_text(segs, &mut plain);
}

fn push_text(segs: &mut Vec<Seg>, plain: &mut String) {
    if !plain.is_empty() {
        segs.push(Seg::Text(std::mem::take(plain)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_and_code_become_links() {
        let p = rv_syn::load_str("fn main() {}\nfn helfer() {}\n");
        let targets = Targets::new(&p);
        let segs = inline(
            "**`main`** at snippet.rs:1, 2 in `snippet::helfer` (other.rs:3)",
            &targets,
        );
        let shown: Vec<String> = segs
            .iter()
            .map(|s| match s {
                Seg::Text(t) => format!("T({t})"),
                Seg::Bold(t) => format!("B({t})"),
                Seg::Code(t, f) => format!("C({t},{})", f.is_some()),
                Seg::Loc(t, _, l) => format!("L({t}={l})"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "C(main,false)",
                "T( at )",
                "L(snippet.rs:1=1)",
                "T(, )",
                "L(2=2)",
                "T( in )",
                "C(snippet::helfer,true)",
                "T( (other.rs:3))",
            ]
        );
    }
}
