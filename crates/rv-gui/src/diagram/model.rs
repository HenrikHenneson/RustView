//! What is computed for the shown function: its analysis, the layout and
//! the wire geometry. Each part is cached by what it depends on, so a frame
//! in which nothing changed computes nothing.

use egui::{Pos2, Vec2};
use rv_ir::{Body, FnId, Project};
use rv_layout::{Layout, Metrics, Orientation};

use super::analysis::Analysis;
use super::fn_view::FnView;
use super::scene::{self, Scene};
use crate::wires::{self, Wire, WireMode};

/// How a function is to be shown: everything the layout and the wires
/// depend on besides the function itself.
pub(super) struct Shown<'a> {
    pub(super) view: &'a FnView,
    pub(super) orientation: Orientation,
    /// Metrics of the label font at zoom 1.
    pub(super) metrics: Metrics,
    pub(super) wire_mode: WireMode,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    func: FnId,
    generation: u64,
    orientation: Orientation,
    version: u64,
    char_width_bits: u32,
}

#[derive(Default)]
pub(super) struct Model {
    /// Analyses of the shown function, per project generation.
    analysis: Option<((FnId, u64), Analysis)>,
    layout: Option<(LayoutKey, Layout)>,
    /// Exit markers for the current layout.
    exit_marks: Option<(LayoutKey, Vec<Pos2>)>,
    /// Wire geometry for the current layout.
    wires: Option<(LayoutKey, Vec<Wire>)>,
}

impl Model {
    /// Brings the model up to date for function `f` of project generation
    /// `generation` and returns what to draw. Wires are not routed while
    /// there is nothing to connect and they are switched off.
    pub(super) fn update<'a>(
        &'a mut self,
        project: &Project,
        f: FnId,
        generation: u64,
        body: &'a Body,
        shown: &Shown,
    ) -> Scene<'a> {
        let key = LayoutKey {
            func: f,
            generation,
            orientation: shown.orientation,
            version: shown.view.version,
            char_width_bits: shown.metrics.char_width.to_bits(),
        };
        let analysis = cached(&mut self.analysis, (f, generation), || {
            Analysis::new(body, project)
        });
        let layout = cached(&mut self.layout, key.clone(), || {
            lay_out(body, analysis, shown)
        });
        let exit_marks = cached(&mut self.exit_marks, key.clone(), || {
            scene::exit_marks(body, layout)
        });
        let wires: &[Wire] = if shown.wire_mode != WireMode::Off || !analysis.specs.is_empty() {
            cached(&mut self.wires, key, || {
                wires::geometry(body, &analysis.specs, layout, shown.orientation)
            })
        } else {
            &[]
        };
        Scene {
            body,
            layout,
            analysis,
            wires,
            exit_marks,
        }
    }

    /// The scene as last computed (with the wires routed last), if `body`
    /// has been shown.
    pub(super) fn scene<'a>(&'a self, body: &'a Body) -> Option<Scene<'a>> {
        let (_, layout) = self.layout.as_ref()?;
        let (_, analysis) = self.analysis.as_ref()?;
        let (_, exit_marks) = self.exit_marks.as_ref()?;
        let wires = self.wires.as_ref().map_or(&[][..], |(_, w)| w.as_slice());
        Some(Scene {
            body,
            layout,
            analysis,
            wires,
            exit_marks,
        })
    }

    /// Size of the diagram as last laid out.
    pub(super) fn layout_size(&self) -> Option<Vec2> {
        self.layout.as_ref().map(|(_, layout)| layout.size)
    }
}

/// The value cached in `slot` for `key`; computed anew when the key changed.
fn cached<K: PartialEq, V>(slot: &mut Option<(K, V)>, key: K, compute: impl FnOnce() -> V) -> &V {
    if slot.as_ref().is_none_or(|(k, _)| *k != key) {
        *slot = Some((key, compute()));
    }
    &slot.as_ref().expect("computed above").1
}

/// Lays out `body` as it is to be shown, with the notes of the analysis.
fn lay_out(body: &Body, analysis: &Analysis, shown: &Shown) -> Layout {
    // Mark structures that show expressions.
    let mut notes = analysis.notes.clone();
    for &s in &shown.view.state.expressions {
        notes
            .entry(s)
            .and_modify(|n| n.push_str(" · ƒx"))
            .or_insert_with(|| "ƒx".into());
    }
    let view = rv_layout::View {
        state: &shown.view.state,
        defaults: &shown.view.defaults,
        orientation: shown.orientation,
    };
    rv_layout::layout(body, view, shown.metrics, &notes)
}
