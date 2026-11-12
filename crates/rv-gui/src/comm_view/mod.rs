//! How threads communicate: a thread × thread matrix (row → column: what
//! flows from the row thread to the column thread) and the details of the
//! selected channel, shared object or join.

use egui::{Rect, RichText, Ui};
use rv_analysis::Program;
use rv_ir::Project;

use crate::style::{CHANNEL, JOIN, SHARED};
use crate::threads::ThreadsAction;

mod details;
mod matrix;
mod svg;

pub use matrix::{CommRef, Matrix};
pub use svg::matrix_svg;

use matrix::{chip, chip_tooltip, thread_label};

/// Share of the width the details panel starts with.
const DETAILS_SHARE: f32 = 0.4;
const DETAILS_MIN_WIDTH: f32 = 220.0;
/// Width the matrix keeps when the details panel is dragged wide.
const MATRIX_MIN_WIDTH: f32 = 150.0;
const GRID_SPACING: [f32; 2] = [14.0, 8.0];
/// How far the grid lines reach beyond the outer cells.
const GRID_OVERHANG: f32 = 6.0;

#[derive(Default)]
pub struct CommView {
    pub selected: Option<CommRef>,
}

/// The rects of the cells drawn, with (row, column); 0 is the header.
type CellRects = Vec<(usize, usize, Rect)>;

impl CommView {
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        program: &Program,
        matrix: &Matrix,
    ) -> ThreadsAction {
        let mut action = ThreadsAction::None;
        if matrix.threads.is_empty() {
            ui.weak("No communication between threads found (no channels, shared objects or joins across threads).");
            return action;
        }
        legend(ui);
        ui.separator();
        // Details in a panel on the right whose width the user can drag;
        // the matrix takes the rest.
        let total = ui.available_width();
        egui::Panel::right("comm_details_panel")
            .resizable(true)
            .default_size(total * DETAILS_SHARE)
            .size_range(DETAILS_MIN_WIDTH..=(total - MATRIX_MIN_WIDTH).max(240.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("comm_details")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let selected = &mut self.selected;
                        if let Some(a) = details::details_ui(ui, project, program, selected) {
                            action = a;
                        }
                    });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            // Chips must not wrap into narrow columns.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            egui::ScrollArea::both()
                .id_salt("comm_matrix")
                .auto_shrink([false, false])
                .show(ui, |ui| self.matrix_ui(ui, project, program, matrix));
        });
        action
    }

    /// The matrix as a grid of chips; clicking a chip selects it.
    fn matrix_ui(&mut self, ui: &mut Ui, project: &Project, program: &Program, matrix: &Matrix) {
        let threads = &matrix.threads;
        // For the lines drawn afterwards.
        let mut cell_rects = CellRects::new();
        egui::Grid::new("comm_grid")
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                header_row(ui, program, threads, &mut cell_rects);
                for (ri, &r) in threads.iter().enumerate() {
                    let rect = ui
                        .label(RichText::new(thread_label(program, r)).strong())
                        .rect;
                    cell_rects.push((ri + 1, 0, rect));
                    for (ci, &c) in threads.iter().enumerate() {
                        let cell = matrix.cell(r, c);
                        let rect = ui
                            .vertical(|ui| self.chips(ui, project, program, cell))
                            .response
                            .rect;
                        cell_rects.push((ri + 1, ci + 1, rect));
                    }
                    ui.end_row();
                }
            });
        draw_grid_lines(ui, &cell_rects, threads.len() + 1);
    }

    /// The communication objects of one cell.
    fn chips(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        program: &Program,
        cell: impl Iterator<Item = CommRef>,
    ) {
        for comm in cell {
            let (text, color) = chip(program, comm);
            let selected = self.selected == Some(comm);
            let label = RichText::new(text).color(color).monospace();
            if ui
                .selectable_label(selected, label)
                .on_hover_text(chip_tooltip(project, program, comm))
                .clicked()
            {
                self.selected = Some(comm);
            }
        }
    }
}

/// What the colors of the chips mean.
fn legend(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label("row → column:");
        ui.label(RichText::new("■ channel (sends to)").color(CHANNEL));
        ui.label(RichText::new("■ shared state (changes, the other uses)").color(SHARED));
        ui.label(RichText::new("■ join (column waits for row to end)").color(JOIN));
    });
}

/// The header row of the matrix: one column per thread.
fn header_row(ui: &mut Ui, program: &Program, threads: &[usize], cell_rects: &mut CellRects) {
    let r = ui.weak("from → to").rect;
    cell_rects.push((0, 0, r));
    for (ci, &c) in threads.iter().enumerate() {
        let r = ui
            .strong(format!("#{c}"))
            .on_hover_text(program.threads[c].title())
            .rect;
        cell_rects.push((0, ci + 1, r));
    }
    ui.end_row();
}

/// Lines between the matrix cells and a gray diagonal (a thread with itself).
fn draw_grid_lines(ui: &Ui, cells: &[(usize, usize, Rect)], n: usize) {
    if cells.is_empty() {
        return;
    }
    let mut col = vec![(f32::INFINITY, f32::NEG_INFINITY); n];
    let mut row = vec![(f32::INFINITY, f32::NEG_INFINITY); n];
    for &(r, c, rect) in cells {
        col[c] = (col[c].0.min(rect.min.x), col[c].1.max(rect.max.x));
        row[r] = (row[r].0.min(rect.min.y), row[r].1.max(rect.max.y));
    }
    // Boundaries halfway between neighboring columns / rows.
    let bounds = |v: &[(f32, f32)]| -> Vec<f32> {
        let mut b = vec![v[0].0 - GRID_OVERHANG];
        for w in v.windows(2) {
            b.push((w[0].1 + w[1].0) / 2.0);
        }
        b.push(v[v.len() - 1].1 + GRID_OVERHANG);
        b
    };
    let (xs, ys) = (bounds(&col), bounds(&row));
    let painter = ui.painter();
    let gray = ui.visuals().weak_text_color().gamma_multiply(0.25);
    for i in 1..n {
        // The thread with itself.
        let r = Rect::from_min_max(egui::pos2(xs[i], ys[i]), egui::pos2(xs[i + 1], ys[i + 1]));
        painter.rect_filled(r, 0.0, gray);
    }
    let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    for &x in &xs {
        painter.line_segment([egui::pos2(x, ys[0]), egui::pos2(x, ys[n])], stroke);
    }
    for &y in &ys {
        painter.line_segment([egui::pos2(xs[0], y), egui::pos2(xs[n], y)], stroke);
    }
}
