//! Exporting views as SVG (built from the layout geometry, so the whole
//! diagram is exported, not just the visible part) or PNG (that SVG rendered
//! with resvg).

use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use egui::{Color32, Pos2, Rect, Vec2};

/// PNGs are rendered at this scale, but never larger than [`MAX_PNG_SIDE`].
const PNG_SCALE: f32 = 2.0;
const MAX_PNG_SIDE: f32 = 16000.0;
const MARGIN: f32 = 16.0;

/// A small SVG writer in diagram coordinates.
pub struct Svg {
    size: Vec2,
    body: String,
}

impl Svg {
    pub fn new(size: Vec2) -> Self {
        Svg {
            size,
            body: String::new(),
        }
    }

    pub fn rect(&mut self, r: Rect, fill: Color32, stroke: Option<(f32, Color32)>, radius: f32) {
        let (sw, sc) = stroke.map_or((0.0, "none".to_string()), |(w, c)| (w, hex(c)));
        let _ = writeln!(
            self.body,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{radius:.1}" fill="{}" fill-opacity="{:.3}" stroke="{sc}" stroke-width="{sw:.1}"/>"#,
            r.min.x + MARGIN,
            r.min.y + MARGIN,
            r.width(),
            r.height(),
            hex(fill),
            alpha(fill),
        );
    }

    /// `pos` is the top-left corner of the text.
    pub fn text(
        &mut self,
        pos: Pos2,
        text: &str,
        size: f32,
        color: Color32,
        mono: bool,
        bold: bool,
    ) {
        self.text_fit(pos, text, size, color, mono, bold, None);
    }

    /// Like [`Self::text`]; with `width` the text is fitted to exactly that
    /// width (layout boxes were measured with the GUI font).
    #[allow(clippy::too_many_arguments)]
    pub fn text_fit(
        &mut self,
        pos: Pos2,
        text: &str,
        size: f32,
        color: Color32,
        mono: bool,
        bold: bool,
        width: Option<f32>,
    ) {
        let family = if mono {
            "Consolas, 'DejaVu Sans Mono', monospace"
        } else {
            "'Segoe UI', 'DejaVu Sans', sans-serif"
        };
        let weight = if bold { "bold" } else { "normal" };
        let fit = width.filter(|w| *w > 1.0).map_or(String::new(), |w| {
            format!(r#" textLength="{w:.1}" lengthAdjust="spacingAndGlyphs""#)
        });
        let _ = writeln!(
            self.body,
            r#"<text x="{:.1}" y="{:.1}" font-family="{family}" font-size="{size:.1}" font-weight="{weight}" fill="{}" dominant-baseline="hanging" xml:space="preserve"{fit}>{}</text>"#,
            pos.x + MARGIN,
            pos.y + MARGIN,
            hex(color),
            escape(text),
        );
    }

    pub fn polyline(&mut self, pts: &[Pos2], width: f32, color: Color32, dash: Option<(f32, f32)>) {
        if pts.len() < 2 {
            return;
        }
        let points: String = pts
            .iter()
            .map(|p| format!("{:.1},{:.1} ", p.x + MARGIN, p.y + MARGIN))
            .collect();
        let dash = dash.map_or(String::new(), |(a, b)| {
            format!(r#" stroke-dasharray="{a} {b}""#)
        });
        let _ = writeln!(
            self.body,
            r#"<polyline points="{points}" fill="none" stroke="{}" stroke-opacity="{:.3}" stroke-width="{width:.1}"{dash}/>"#,
            hex(color),
            alpha(color),
        );
    }

    pub fn polygon(&mut self, pts: &[Pos2], fill: Color32) {
        let points: String = pts
            .iter()
            .map(|p| format!("{:.1},{:.1} ", p.x + MARGIN, p.y + MARGIN))
            .collect();
        let _ = writeln!(
            self.body,
            r#"<polygon points="{points}" fill="{}" fill-opacity="{:.3}"/>"#,
            hex(fill),
            alpha(fill)
        );
    }

    /// Line with an arrow head at the end.
    pub fn arrow(&mut self, pts: &[Pos2], width: f32, color: Color32, dash: Option<(f32, f32)>) {
        self.polyline(pts, width, color, dash);
        let n = pts.len();
        if n < 2 {
            return;
        }
        let dir = (pts[n - 1] - pts[n - 2]).normalized();
        if !dir.is_finite() {
            return;
        }
        let size = 5.0 + width;
        let side = egui::vec2(-dir.y, dir.x) * (size * 0.5);
        let back = pts[n - 1] - dir * size;
        self.polygon(&[pts[n - 1], back + side, back - side], color);
    }

    pub fn finish(self) -> String {
        let (w, h) = (self.size.x + 2.0 * MARGIN, self.size.y + 2.0 * MARGIN);
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">\n<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\n{}</svg>\n",
            self.body
        )
    }
}

fn hex(c: Color32) -> String {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn alpha(c: Color32) -> f32 {
    c.to_srgba_unmultiplied()[3] as f32 / 255.0
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Ask where to save; the extension (`.svg` / `.png`) decides the format.
pub fn ask_path(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export")
        .set_file_name(format!("{default_name}.svg"))
        .add_filter("SVG (vector)", &["svg"])
        .add_filter("PNG (image)", &["png"])
        .save_file()
}

/// Write `svg` to `path` as SVG or PNG (by extension).
pub fn save(svg: &str, path: &Path) -> Result<()> {
    let png = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !png {
        return std::fs::write(path, svg).with_context(|| format!("writing {}", path.display()));
    }
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &opt).context("parsing the generated SVG")?;
    let size = tree.size();
    let scale = PNG_SCALE.min(MAX_PNG_SIDE / size.width().max(size.height()));
    let (w, h) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).context("image too large")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .save_png(path)
        .with_context(|| format!("writing {}", path.display()))
}

/// Ask for a file and save; returns a status message.
pub fn export(default_name: &str, svg: impl FnOnce() -> String) -> Option<String> {
    let path = ask_path(default_name)?;
    Some(match save(&svg(), &path) {
        Ok(()) => format!("exported {}", path.display()),
        Err(e) => format!("export failed: {e:#}"),
    })
}

/// File-name friendly version of a path like `mtd::ec::task::sitzung`.
pub fn file_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_renders_to_png() {
        let mut svg = Svg::new(egui::vec2(200.0, 80.0));
        svg.rect(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(120.0, 30.0)),
            Color32::LIGHT_BLUE,
            Some((1.0, Color32::BLUE)),
            4.0,
        );
        svg.text(
            egui::pos2(6.0, 6.0),
            "a < b & c",
            12.0,
            Color32::BLACK,
            true,
            false,
        );
        svg.arrow(
            &[egui::pos2(120.0, 15.0), egui::pos2(190.0, 60.0)],
            1.5,
            Color32::RED,
            Some((4.0, 2.0)),
        );
        let text = svg.finish();
        assert!(text.contains("a &lt; b &amp; c"));
        let dir = std::env::temp_dir().join(format!("rv-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("t.png");
        save(&text, &png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        std::fs::remove_dir_all(&dir).ok();
    }
}
