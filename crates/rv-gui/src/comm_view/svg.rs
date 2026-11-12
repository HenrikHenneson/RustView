//! The communication matrix as SVG table, for the export.

use egui::{Color32, Rect, Vec2, pos2, vec2};
use rv_analysis::Program;

use super::matrix::{Matrix, chip, thread_label};
use crate::export::Svg;

/// Estimated width of a character.
const CH: f32 = 7.0;
const LINE: f32 = 16.0;
const PAD: f32 = 8.0;
const FONT: f32 = 12.0;
/// Space between the top of a cell and its text.
const TEXT_TOP: f32 = 4.0;
/// Height of the header row.
const HEAD_H: f32 = LINE + PAD;
const HEAD_COLOR: Color32 = Color32::from_gray(30);
/// The diagonal: a thread with itself.
const SELF_COLOR: Color32 = Color32::from_gray(225);
const GRID_COLOR: Color32 = Color32::from_gray(200);

/// Sizes of the table.
struct Table {
    col_w: Vec<f32>,
    row_h: Vec<f32>,
    /// Left edge of the table, then of every column (the first one, after
    /// the thread names, at index 1), then the right edge.
    xs: Vec<f32>,
    /// Top edge of the table, then of every row (the first one, below the
    /// header, at index 1), then the bottom edge.
    ys: Vec<f32>,
    /// Size of the whole table.
    size: Vec2,
}

impl Table {
    fn new(program: &Program, matrix: &Matrix) -> Self {
        let threads = &matrix.threads;
        let head_w = threads
            .iter()
            .map(|&t| thread_label(program, t).chars().count())
            .max()
            .unwrap_or(10) as f32
            * CH
            + 2.0 * PAD;
        let col_w: Vec<f32> = threads
            .iter()
            .map(|&c| {
                let widest = threads
                    .iter()
                    .flat_map(|&r| matrix.cell(r, c))
                    .map(|x| chip(program, x).0.chars().count())
                    .max()
                    .unwrap_or(3);
                widest.max(3) as f32 * CH + 2.0 * PAD
            })
            .collect();
        let row_h: Vec<f32> = threads
            .iter()
            .map(|&r| {
                let most = threads
                    .iter()
                    .map(|&c| matrix.cell(r, c).count())
                    .max()
                    .unwrap_or(0);
                most.max(1) as f32 * LINE + PAD
            })
            .collect();
        let mut xs = vec![0.0, head_w];
        for w in &col_w {
            xs.push(xs[xs.len() - 1] + w);
        }
        let mut ys = vec![0.0, HEAD_H];
        for h in &row_h {
            ys.push(ys[ys.len() - 1] + h);
        }
        let width = head_w + col_w.iter().sum::<f32>();
        let height = HEAD_H + row_h.iter().sum::<f32>();
        Table {
            col_w,
            row_h,
            xs,
            ys,
            size: vec2(width, height),
        }
    }
}

/// The communication matrix as SVG table.
pub fn matrix_svg(program: &Program, matrix: &Matrix) -> String {
    let table = Table::new(program, matrix);
    let mut svg = Svg::new(table.size);
    for (ci, &c) in matrix.threads.iter().enumerate() {
        let pos = pos2(table.xs[ci + 1] + PAD, TEXT_TOP);
        svg.text(pos, &format!("#{c}"), FONT, HEAD_COLOR, false, true);
    }
    for (ri, &r) in matrix.threads.iter().enumerate() {
        let pos = pos2(PAD, table.ys[ri + 1] + TEXT_TOP);
        let label = thread_label(program, r);
        svg.text(pos, &label, FONT, HEAD_COLOR, false, true);
        row(&mut svg, program, matrix, &table, ri);
    }
    grid(&mut svg, &table);
    svg.finish()
}

/// The cells of the row with index `ri`.
fn row(svg: &mut Svg, program: &Program, matrix: &Matrix, table: &Table, ri: usize) {
    let r = matrix.threads[ri];
    let y = table.ys[ri + 1];
    for (ci, &c) in matrix.threads.iter().enumerate() {
        let x = table.xs[ci + 1];
        if r == c {
            let cell = Rect::from_min_size(pos2(x, y), vec2(table.col_w[ci], table.row_h[ri]));
            svg.rect(cell, SELF_COLOR, None, 0.0);
        }
        for (k, comm) in matrix.cell(r, c).enumerate() {
            let (text, color) = chip(program, comm);
            let pos = pos2(x + PAD, y + TEXT_TOP + k as f32 * LINE);
            svg.text(pos, &text, FONT, color, true, false);
        }
    }
}

/// The lines between the columns and rows.
fn grid(svg: &mut Svg, table: &Table) {
    let (width, height) = (table.size.x, table.size.y);
    for &x in &table.xs {
        svg.polyline(&[pos2(x, 0.0), pos2(x, height)], 1.0, GRID_COLOR, None);
    }
    for &y in &table.ys {
        svg.polyline(&[pos2(0.0, y), pos2(width, y)], 1.0, GRID_COLOR, None);
    }
}
