//! What the graph views (thread graph, module graph) share: a canvas that is
//! moved and zoomed with the mouse, and curved arrows on it.

use egui::{Painter, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2, vec2};

/// Zoom range of the mouse wheel.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 3.0;
/// A curve counts as hovered when the pointer is within this many screen points.
const HOVER_DISTANCE: f32 = 6.0;
/// Length of an arrow head in addition to the width of its line.
const ARROW_HEAD: f32 = 6.0;

/// Position and scale of a graph on its canvas.
#[derive(Clone, Copy)]
pub struct PanZoom {
    pub zoom: f32,
    pub pan: Vec2,
}

impl Default for PanZoom {
    fn default() -> Self {
        PanZoom {
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

impl PanZoom {
    /// Dragging moves the graph, Ctrl+wheel zooms around the pointer and the
    /// wheel alone scrolls. `response` is the one of the canvas `rect`.
    pub fn handle_input(&mut self, ui: &Ui, rect: Rect, response: &Response) {
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if !response.hovered() {
            return;
        }
        let (zoom_delta, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
        if zoom_delta != 1.0
            && let Some(p) = response.hover_pos()
        {
            let world = (p - rect.min - self.pan) / self.zoom;
            self.zoom = (self.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            self.pan = p - rect.min - world * self.zoom;
        } else {
            self.pan += scroll;
        }
    }

    /// The mapping between graph and screen coordinates on the canvas `rect`.
    pub fn transform(&self, rect: Rect) -> Transform {
        Transform {
            zoom: self.zoom,
            origin: rect.min + self.pan,
        }
    }
}

/// Mapping between graph ("world") and screen coordinates.
#[derive(Clone, Copy)]
pub struct Transform {
    pub zoom: f32,
    /// Screen position of the graph's origin.
    pub origin: Pos2,
}

impl Transform {
    pub fn to_screen(self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    pub fn to_world(self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }

    pub fn rect_to_screen(self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }

    pub fn line_to_screen(self, points: &[Pos2]) -> Vec<Pos2> {
        points.iter().map(|&p| self.to_screen(p)).collect()
    }
}

/// Points on a cubic Bézier curve: `steps` segments from `p[0]` to `p[3]`.
pub fn bezier(p: &[Pos2; 4], steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            (p[0].to_vec2() * (u * u * u)
                + p[1].to_vec2() * (3.0 * u * u * t)
                + p[2].to_vec2() * (3.0 * u * t * t)
                + p[3].to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
}

/// Distance of `p` from the segment `a`–`b`.
fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// Index of the line closest to the pointer `p`, if one is near enough to
/// count as hovered. Lines and `p` are in graph coordinates.
pub fn hovered_line<'a>(
    lines: impl Iterator<Item = &'a [Pos2]>,
    p: Pos2,
    zoom: f32,
) -> Option<usize> {
    lines
        .enumerate()
        .filter_map(|(i, line)| {
            let d = line
                .windows(2)
                .map(|s| distance_to_segment(p, s[0], s[1]))
                .fold(f32::INFINITY, f32::min);
            (d < HOVER_DISTANCE / zoom).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Arrow head at the end of the line `pts` (screen coordinates) of this stroke.
pub fn paint_arrow_head(painter: &Painter, pts: &[Pos2], stroke: Stroke) {
    let n = pts.len();
    let dir = (pts[n - 1] - pts[n - 2]).normalized();
    if !dir.is_finite() {
        return;
    }
    let size = ARROW_HEAD + stroke.width;
    let side = vec2(-dir.y, dir.x) * (size * 0.5);
    let back = pts[n - 1] - dir * size;
    painter.add(Shape::convex_polygon(
        vec![pts[n - 1], back + side, back - side],
        stroke.color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn transform_maps_there_and_back() {
        let view = PanZoom {
            zoom: 2.0,
            pan: vec2(10.0, 20.0),
        };
        let t = view.transform(Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0)));
        let screen = t.to_screen(pos2(3.0, 4.0));
        assert_eq!(screen, pos2(116.0, 78.0));
        assert_eq!(t.to_world(screen), pos2(3.0, 4.0));
    }

    #[test]
    fn bezier_runs_from_first_to_last_point() {
        let p = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(20.0, 30.0),
            pos2(30.0, 30.0),
        ];
        let line = bezier(&p, 8);
        assert_eq!(line.len(), 9);
        assert_eq!((line[0], line[8]), (p[0], p[3]));
    }

    #[test]
    fn the_closest_line_within_reach_is_hovered() {
        let near = [pos2(0.0, 0.0), pos2(100.0, 0.0)];
        let nearer = [pos2(0.0, 5.0), pos2(100.0, 5.0)];
        let lines = [near.as_slice(), nearer.as_slice()];
        let hovered = |p, zoom| hovered_line(lines.iter().copied(), p, zoom);
        assert_eq!(hovered(pos2(50.0, 4.0), 1.0), Some(1));
        assert_eq!(hovered(pos2(50.0, -3.0), 1.0), Some(0));
        assert_eq!(hovered(pos2(50.0, 20.0), 1.0), None);
        // Zoomed in, the same distance on screen is a shorter one in the graph.
        assert_eq!(hovered(pos2(50.0, -3.0), 4.0), None);
    }
}
