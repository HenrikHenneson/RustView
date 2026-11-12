//! The "by function" mode on screen: one row per spawning function, one
//! card per lane.

use egui::{Color32, RichText, Stroke, Ui};
use rv_analysis::End;
use rv_ir::{FnId, Project};

use super::ThreadsAction;
use super::overview::{Card, Group, Overview};
use crate::style::{SHARED, THREAD};
use crate::widgets::mono_link;

/// Colors to tell the channels of a function apart.
const CHANNEL_COLORS: [Color32; 6] = [
    Color32::from_rgb(0, 150, 165),
    Color32::from_rgb(210, 110, 0),
    Color32::from_rgb(60, 110, 220),
    Color32::from_rgb(190, 60, 140),
    Color32::from_rgb(90, 140, 30),
    Color32::from_rgb(120, 90, 200),
];
const CARD_WIDTH: f32 = 280.0;
/// Space above a section of a card.
const SECTION_SPACE: f32 = 4.0;
/// Functions listed under "runs".
const MAX_RUNS: usize = 6;

pub(super) fn overview_ui(ui: &mut Ui, project: &Project, o: &Overview) -> ThreadsAction {
    let mut action = ThreadsAction::None;
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!(
            "{} threads/tasks started in {} functions · {} channels · {} shared-state groups",
            o.lanes,
            o.groups.len(),
            o.channels,
            o.shared
        ));
        ui.weak("· recognized by API name (thread::spawn/Builder, scope, tokio/smol/async-std, rayon, join!/select!)");
    });
    ui.separator();
    if o.groups.is_empty() {
        ui.weak("No threads or tasks found.");
        return action;
    }

    egui::ScrollArea::vertical()
        .id_salt("threads_scroll")
        .show(ui, |ui| {
            for g in &o.groups {
                if let Some(a) = group_ui(ui, project, g) {
                    action = a;
                }
                ui.add_space(12.0);
            }
        });
    action
}

/// A spawning function: its name and a row with its cards.
fn group_ui(ui: &mut Ui, project: &Project, g: &Group) -> Option<ThreadsAction> {
    let mut action = None;
    let func = project.function(g.func);
    ui.horizontal(|ui| {
        if ui
            .link(RichText::new(&func.qual_path).monospace().strong())
            .on_hover_text("Show the diagram")
            .clicked()
        {
            action = Some(ThreadsAction::ShowFn(g.func));
        }
        ui.weak(format!("line {}", func.name_span.start.line));
    });
    egui::ScrollArea::horizontal()
        .id_salt(("threads_row", g.func))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for (i, card) in g.cards.iter().enumerate() {
                    if i == 1 {
                        ui.label(RichText::new("‖").size(28.0).weak());
                    }
                    action = card_ui(ui, g.func, card).or(action.take());
                }
            });
        });
    action
}

fn card_ui(ui: &mut Ui, func: FnId, card: &Card) -> Option<ThreadsAction> {
    let mut action = None;
    let stroke = if card.parallel {
        Stroke::new(2.0, THREAD)
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    egui::Frame::group(ui.style())
        .stroke(stroke)
        .inner_margin(8.0)
        .show(ui, |ui| {
            // The row is horizontal; the card's content is a column.
            ui.vertical(|ui| {
                ui.set_width(CARD_WIDTH);
                ui.strong(&card.title);
                if let Some(node) = card.node
                    && ui.link(format!("started at line {}", card.line)).clicked()
                {
                    action = Some(ThreadsAction::ShowNode(func, node));
                }
                action = card_runs(ui, card).or(action.take());
                card_channels(ui, card);
                card_shared(ui, card);
            });
        });
    action
}

/// The functions a lane runs, as links where they are known.
fn card_runs(ui: &mut Ui, card: &Card) -> Option<ThreadsAction> {
    let mut action = None;
    if card.runs.is_empty() {
        return action;
    }
    ui.add_space(SECTION_SPACE);
    ui.weak("runs");
    for (name, target) in card.runs.iter().take(MAX_RUNS) {
        match target {
            Some(f) => {
                if mono_link(ui, name).clicked() {
                    action = Some(ThreadsAction::ShowFn(*f));
                }
            }
            None => {
                ui.label(RichText::new(name).monospace().weak());
            }
        }
    }
    action
}

fn card_channels(ui: &mut Ui, card: &Card) {
    if card.channels.is_empty() {
        return;
    }
    ui.add_space(SECTION_SPACE);
    ui.weak("channels");
    for (name, color, end, op) in &card.channels {
        let arrow = match end {
            End::Send => "→ sends to",
            End::Recv => "← receives from",
        };
        let op = op
            .as_deref()
            .map_or_else(|| " (passed on)".to_string(), |o| format!(" ({o})"));
        let color = CHANNEL_COLORS[color % CHANNEL_COLORS.len()];
        ui.label(RichText::new(format!("{arrow} {name}{op}")).color(color));
    }
}

fn card_shared(ui: &mut Ui, card: &Card) {
    if card.shared.is_empty() {
        return;
    }
    ui.add_space(SECTION_SPACE);
    ui.weak("shared state");
    for name in &card.shared {
        ui.label(RichText::new(name).monospace().color(SHARED));
    }
}
