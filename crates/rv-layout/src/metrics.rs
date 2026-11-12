//! Font metrics the layout measures labels with.

/// Font metrics of the (monospace) label font at zoom 1.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub char_width: f32,
    pub line_height: f32,
    /// Longer labels are cut with `…`.
    pub max_label_chars: usize,
}

impl Metrics {
    /// Width of `s` in the label font.
    pub(crate) fn text(&self, s: &str) -> f32 {
        s.chars().count() as f32 * self.char_width
    }

    /// Width of the expand/collapse triangle and the case arrows.
    pub(crate) fn icon(&self) -> f32 {
        self.line_height
    }
}
