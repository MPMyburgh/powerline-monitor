use std::time::SystemTime;

use eframe::egui::{self, Key, ViewportCommand};
use powerline_core::events::Severity;
use powerline_core::model::Mac;

use crate::poller::Shared;
use crate::{App, Tab};

pub fn handle(ctx: &egui::Context, app: &mut App, s: &Shared) {
    app.health.observe(s);
    let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
    if pressed(Key::Escape) {
        app.health.editing = None;
    }
    if ctx.text_edit_focused() {
        return;
    }
    for (key, tab) in [
        (Key::Num1, Tab::Overview),
        (Key::Num2, Tab::Speeds),
        (Key::Num3, Tab::History),
        (Key::Num4, Tab::Events),
        (Key::Num5, Tab::Settings),
        (Key::Num6, Tab::Adapter),
    ] {
        if pressed(key) {
            app.tab = tab;
        }
    }
    if pressed(Key::R) {
        app.refresh.send(()).ok();
    }
    if pressed(Key::D) {
        app.health.show_details = !app.health.show_details;
    }
    if pressed(Key::W) {
        app.health.min_severity = match app.health.min_severity {
            Severity::Info => Severity::Warning,
            Severity::Warning => Severity::Critical,
            Severity::Critical => Severity::Info,
        };
    }
    if pressed(Key::Slash) {
        app.tab = Tab::Events;
        app.health.focus_filter = true;
    }
    if pressed(Key::Space) {
        app.health.paused = match app.health.paused {
            Some(_) => None,
            None => Some(SystemTime::now()),
        };
    }
    if pressed(Key::Q) {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let step = if pressed(Key::ArrowRight) {
        1
    } else if pressed(Key::ArrowLeft) {
        -1
    } else {
        0
    };
    if step != 0 {
        if let Some(st) = &s.state {
            app.selected = neighbour(&st.macs(), app.selected, step);
        }
    }
}

/// The directed link `step` places after `current` in macs order, wrapping; the first link when nothing is selected.
fn neighbour(macs: &[Mac], current: Option<(Mac, Mac)>, step: isize) -> Option<(Mac, Mac)> {
    let links: Vec<(Mac, Mac)> = macs.iter().flat_map(|a| macs.iter().filter(move |b| *b != a).map(move |b| (*a, *b))).collect();
    if links.is_empty() {
        return None;
    }
    let i = current
        .and_then(|c| links.iter().position(|l| *l == c))
        .map_or(0, |i| (i as isize + step).rem_euclid(links.len() as isize) as usize);
    Some(links[i])
}
