//! Read-only source panel with highlighting, line numbers and jump targets.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Sense};
use rv_ir::{FileId, Project, Span};

use crate::highlight::Highlighter;

const TAB: &str = "    ";

#[derive(Default)]
pub struct CodeView {
    pub file: Option<FileId>,
    /// Lines shown with a background (e.g. the selected function).
    pub region: Option<Span>,
    /// Exact range shown with a stronger background (e.g. its name).
    pub mark: Option<Span>,
    /// Clicked line, 1-based.
    pub cursor_line: Option<u32>,
    scroll_to: Option<u32>,
    /// Longest line of the current file in chars, for horizontal scrolling.
    max_cols: usize,
}

pub enum CodeAction {
    None,
    /// Double click: open in the external editor (line 1-based, col 0-based).
    OpenInEditor {
        line: u32,
        col: u32,
    },
}

impl CodeView {
    pub fn show_span(&mut self, project: &Project, region: Span, mark: Span) {
        self.set_file(project, region.file);
        self.scroll_to = Some(region.start.line);
        self.cursor_line = None;
        self.region = Some(region);
        self.mark = Some(mark);
    }

    /// Show `file` with new marks but keep the scroll position (for reloads).
    pub fn update_in_place(
        &mut self,
        project: &Project,
        file: FileId,
        region: Option<Span>,
        mark: Option<Span>,
    ) {
        self.file = None; // the text may have changed: recompute line widths
        self.set_file(project, file);
        self.region = region;
        self.mark = mark;
    }

    pub fn show_file(&mut self, project: &Project, file: FileId) {
        self.set_file(project, file);
        self.scroll_to = Some(1);
        self.region = None;
        self.mark = None;
        self.cursor_line = None;
    }

    fn set_file(&mut self, project: &Project, file: FileId) {
        if self.file != Some(file) {
            self.file = Some(file);
            self.max_cols = project
                .file(file)
                .text
                .lines()
                .map(|l| l.chars().count() + l.matches('\t').count() * (TAB.len() - 1))
                .max()
                .unwrap_or(0);
        }
    }

    pub fn clear(&mut self) {
        *self = CodeView::default();
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        highlighter: &mut Highlighter,
    ) -> CodeAction {
        let Some(file_id) = self.file else {
            ui.weak("Select a function to see its source.");
            return CodeAction::None;
        };
        let file = project.file(file_id);
        let shown_path = file
            .path
            .strip_prefix(&project.root)
            .unwrap_or(&file.path)
            .display()
            .to_string();
        ui.horizontal(|ui| {
            ui.strong(shown_path);
            if let Some(r) = &self.region {
                ui.weak(format!("lines {}–{}", r.start.line, r.end.line));
            }
        });
        if let Some(err) = &file.parse_error {
            ui.colored_label(ui.visuals().error_fg_color, format!("parse error: {err}"));
        }
        ui.separator();

        let dark = ui.visuals().dark_mode;
        let styles = highlighter.get(&file.text, dark);
        let text: Arc<str> = file.text.clone();
        let line_count = file.lines.line_count();

        let font = FontId::monospace(13.0);
        let row_height = ui.fonts_mut(|f| f.row_height(&font));
        let char_width = ui.fonts_mut(|f| f.glyph_width(&font, 'x'));
        let gutter_chars = line_count.to_string().len() + 2;
        let gutter = gutter_chars as f32 * char_width;
        let content_width = gutter + (self.max_cols + 2) as f32 * char_width;

        let visuals = ui.visuals().clone();
        let default_color = visuals.text_color();
        let region_bg = visuals.selection.bg_fill.gamma_multiply(0.25);
        let mark_bg = visuals.selection.bg_fill.gamma_multiply(0.7);
        let cursor_bg = visuals.faint_bg_color;

        let mut scroll = egui::ScrollArea::both()
            .id_salt("code_scroll")
            .auto_shrink(false);
        if let Some(line) = self.scroll_to.take() {
            let spacing = ui.spacing().item_spacing.y;
            let target = (line.saturating_sub(4)) as f32 * (row_height + spacing);
            scroll = scroll.vertical_scroll_offset(target);
        }

        let mut action = CodeAction::None;
        scroll.show_rows(ui, row_height, line_count, |ui, rows| {
            ui.set_min_width(content_width.max(ui.available_width()));
            for i in rows {
                let line_no = i as u32 + 1;
                let byte_range = file.lines.line_range(&text, line_no);
                let line_text = &text[byte_range.clone()];
                let width = ui.available_width().max(content_width);
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(width, row_height), Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter();

                let in_region = self
                    .region
                    .as_ref()
                    .is_some_and(|r| (r.start.line..=r.end.line).contains(&line_no));
                if self.cursor_line == Some(line_no) {
                    painter.rect_filled(rect, 0.0, cursor_bg);
                } else if in_region {
                    painter.rect_filled(rect, 0.0, region_bg);
                }
                if in_region {
                    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height()));
                    painter.rect_filled(bar, 0.0, visuals.selection.stroke.color);
                }

                painter.text(
                    rect.min + egui::vec2(gutter - char_width, 0.0),
                    egui::Align2::RIGHT_TOP,
                    line_no.to_string(),
                    font.clone(),
                    visuals.weak_text_color(),
                );

                let mark = self
                    .mark
                    .as_ref()
                    .filter(|m| m.file == file_id)
                    .map(|m| {
                        let start = (m.bytes.start as usize).saturating_sub(byte_range.start);
                        let end = (m.bytes.end as usize).saturating_sub(byte_range.start);
                        start..end
                    })
                    .filter(|r| r.end > 0 && r.start < line_text.len().max(1));
                let line_styles = styles.as_ref().and_then(|s| s.lines.get(i));
                let job = line_job(line_text, line_styles, &font, default_color, mark, mark_bg);
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                painter.galley(rect.min + egui::vec2(gutter, 0.0), galley, default_color);

                if resp.clicked() {
                    self.cursor_line = Some(line_no);
                }
                if resp.double_clicked() {
                    let col = resp
                        .interact_pointer_pos()
                        .map(|p| ((p.x - rect.min.x - gutter) / char_width).max(0.0) as usize)
                        .map_or(0, |visual| visual_to_char_col(line_text, visual));
                    action = CodeAction::OpenInEditor {
                        line: line_no,
                        col: col as u32,
                    };
                }
            }
        });
        action
    }
}

fn line_job(
    line: &str,
    styles: Option<&crate::highlight::LineStyles>,
    font: &FontId,
    default_color: Color32,
    mark: Option<std::ops::Range<usize>>,
    mark_bg: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut push = |piece: &str, color: Color32, italic: bool, marked: bool| {
        if piece.is_empty() {
            return;
        }
        let format = TextFormat {
            font_id: font.clone(),
            color,
            italics: italic,
            background: if marked {
                mark_bg
            } else {
                Color32::TRANSPARENT
            },
            ..Default::default()
        };
        job.append(&piece.replace('\t', TAB), 0.0, format);
    };
    // Split every styled piece at the mark boundaries.
    let pieces: Vec<(std::ops::Range<usize>, Color32, bool)> = match styles {
        Some(s) if !s.is_empty() => s
            .iter()
            .map(|(r, c, i)| (r.start.min(line.len())..r.end.min(line.len()), *c, *i))
            .collect(),
        _ => vec![(0..line.len(), default_color, false)],
    };
    for (range, color, italic) in pieces {
        match &mark {
            Some(m) => {
                let cuts = [
                    range.start,
                    m.start.clamp(range.start, range.end),
                    m.end.clamp(range.start, range.end),
                    range.end,
                ];
                for w in cuts.windows(2) {
                    let marked = w[0] >= m.start && w[1] <= m.end && w[0] < w[1];
                    push(line.get(w[0]..w[1]).unwrap_or(""), color, italic, marked);
                }
            }
            None => push(line.get(range).unwrap_or(""), color, italic, false),
        }
    }
    job
}

/// Map a visual column (tabs expanded) to a char column.
fn visual_to_char_col(line: &str, visual: usize) -> usize {
    let mut v = 0;
    for (i, c) in line.chars().enumerate() {
        v += if c == '\t' { TAB.len() } else { 1 };
        if v > visual {
            return i;
        }
    }
    line.chars().count()
}
