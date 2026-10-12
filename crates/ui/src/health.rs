use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Align2, Margin, RichText, Stroke};
use powerline_core::events::Severity;
use powerline_core::model::{Mac, NetState};

use crate::poller::Shared;
use crate::{theme, App};

pub struct HealthState {
    pub show_details: bool,
    pub explain_key: bool,
    pub selected_adapter: Option<Mac>,
    pub min_severity: Severity,
    pub filter: String,
    pub focus_filter: bool,
    pub editing: Option<(Mac, String)>,
    pub dismissed_naming: bool,
    pub paused: Option<SystemTime>,
    pub toast: Option<(String, Instant)>,
    pub last_seen: BTreeMap<Mac, SystemTime>,
    pub first_seen: BTreeMap<Mac, SystemTime>,
    observed: Option<SystemTime>,
}

impl Default for HealthState {
    fn default() -> HealthState {
        HealthState {
            show_details: false,
            explain_key: false,
            selected_adapter: None,
            min_severity: Severity::Info,
            filter: String::new(),
            focus_filter: false,
            editing: None,
            dismissed_naming: false,
            paused: None,
            toast: None,
            last_seen: BTreeMap::new(),
            first_seen: BTreeMap::new(),
            observed: None,
        }
    }
}

impl HealthState {
    /// Records when each adapter last answered; does its work once per new poll.
    pub fn observe(&mut self, s: &Shared) {
        let Some(st) = &s.state else { return };
        if self.observed == Some(st.at) {
            return;
        }
        self.observed = Some(st.at);
        for d in &st.devices {
            self.last_seen.insert(d.mac, st.at);
            self.first_seen.entry(d.mac).or_insert(st.at);
        }
    }

    /// Adapters seen earlier in this session but absent from `st`, with when they last answered.
    pub fn not_answering(&self, st: &NetState) -> Vec<(Mac, SystemTime)> {
        self.last_seen.iter().filter(|(m, _)| !st.devices.iter().any(|d| d.mac == **m)).map(|(m, t)| (*m, *t)).collect()
    }
}

pub fn copy(ctx: &egui::Context, app: &mut App, text: String) {
    ctx.copy_text(text);
    app.health.toast = Some(("Copied".to_string(), Instant::now()));
}

pub fn draw_toast(ctx: &egui::Context, app: &mut App) {
    let Some((text, at)) = &app.health.toast else { return };
    if at.elapsed() > Duration::from_secs(2) {
        app.health.toast = None;
        return;
    }
    let p = theme::palette(ctx);
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .anchor(Align2::RIGHT_BOTTOM, [-16.0, -40.0])
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(p.tile)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| ui.label(RichText::new(text).strong()));
        });
    ctx.request_repaint_after(Duration::from_millis(200));
}
