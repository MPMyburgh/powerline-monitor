use eframe::egui::{self, CursorIcon, RichText, Sense, Ui, ViewportCommand};
use powerline_core::events::Severity;
use powerline_core::model::{Mac, Role};
use powerline_core::render::{self, Verdict};
use powerline_core::text;

use crate::matrix::{self, Class};
use crate::poller::Shared;
use crate::{theme, widgets, App, Tab};

/// The verdict for the latest poll, overridden while the adapters cannot be reached; also sets the window title.
pub fn verdict(ctx: &egui::Context, s: &Shared) -> Option<Verdict> {
    let mut v = s.state.as_ref().filter(|st| !st.devices.is_empty()).map(|st| render::verdict(st, &s.cfg, Some(&s.tracker)));
    if let Some(e) = &s.error {
        v = Some(Verdict {
            severity: Some(Severity::Warning),
            line: format!("PROBLEM  Can't reach the adapters on {}: {e}", s.cfg.effective_interfaces().join(", ")),
            title: "Powerline: no answer".to_string(),
        });
    }
    if let Some(v) = &v {
        ctx.send_viewport_cmd(ViewportCommand::Title(v.title.clone()));
    }
    v
}

pub fn show(ui: &mut Ui, app: &mut App, s: &mut Shared) {
    let Some(st) = s.state.as_ref().filter(|st| !st.devices.is_empty()) else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Looking for powerline adapters on {}…", s.cfg.effective_interfaces().join(", ")));
        });
        return;
    };
    let p = theme::palette(ui.ctx());
    let cfg = &s.cfg;
    let name = |m: Mac| cfg.name(m);
    let macs = st.macs();
    let ring = &s.ring;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(RichText::new("Links, slowest first").strong());
        let mut links: Vec<(Mac, Mac, Option<u16>, Option<u16>)> = Vec::new();
        for (i, a) in macs.iter().enumerate() {
            for b in &macs[i + 1..] {
                let (ab, ba) = (st.rate(*a, *b).map(|r| r.0), st.rate(*b, *a).map(|r| r.0));
                if ab.is_some() || ba.is_some() {
                    links.push((*a, *b, ab, ba));
                }
            }
        }
        links.sort_by_key(|(_, _, ab, ba)| ab.unwrap_or(u16::MAX).min(ba.unwrap_or(u16::MAX)));
        if links.is_empty() {
            ui.label(RichText::new("No link has reported a speed yet").weak());
        }
        let figure = |r: Option<u16>| r.map_or("?".to_string(), |r| r.to_string());
        egui::Grid::new("links").spacing([14.0, 4.0]).show(ui, |ui| {
            for (a, b, ab, ba) in &links {
                let worst = [(a, b, ab), (b, a, ba)]
                    .into_iter()
                    .filter_map(|(x, y, r)| r.map(|r| matrix::classify(cfg, ring, *x, *y, r)))
                    .max();
                let (class, colour) = match worst {
                    Some(Class::Poor) => ("poor !", theme::POOR),
                    Some(Class::Reduced) => ("reduced ↓", theme::REDUCED),
                    _ => ("ok", theme::GOOD),
                };
                let r = ui.add(egui::Label::new(format!("{} ↔ {}", name(*a), name(*b))).sense(Sense::click()))
                    | ui.add(egui::Label::new(RichText::new(format!("{} / {}", figure(*ab), figure(*ba))).monospace()).sense(Sense::click()))
                    | ui.add(egui::Label::new(RichText::new(class).color(colour).strong()).sense(Sense::click()));
                if r.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    app.selected = Some(if ab.unwrap_or(u16::MAX) <= ba.unwrap_or(u16::MAX) { (*a, *b) } else { (*b, *a) });
                    app.tab = Tab::History;
                }
                ui.end_row();
            }
        });

        ui.add_space(10.0);
        ui.label(RichText::new("Adapters").strong());
        for mac in &macs {
            let d = st.devices.iter().find(|d| d.mac == *mac).unwrap();
            let snap = st.snapshot(*mac);
            ui.horizontal(|ui| {
                widgets::role_badge(ui, snap.map_or(Role::Unknown, |s| s.role), &p);
                ui.label(RichText::new(name(*mac)).strong());
                if d.local {
                    ui.label(RichText::new("wired to this computer").color(p.muted));
                }
                if let Some(up) = snap.and_then(|s| s.uptime_secs) {
                    ui.label(RichText::new(format!("up {}", text::duration(up.into()))).weak());
                }
            });
        }
        for (mac, since) in app.health.not_answering(st) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name(mac)).color(p.muted));
                ui.label(RichText::new(format!("not answering since {}", text::stamp(since))).color(p.warn));
            });
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Recent").strong());
        if s.events.is_empty() {
            ui.label(RichText::new("Nothing has changed since the window opened").weak());
        }
        for e in s.events.iter().rev().take(5) {
            ui.label(format!("{}  {}", text::stamp(e.at()), e.summary(&name)));
        }

        ui.add_space(14.0);
        let mut footer = "1-6 tabs · R refresh · D details · / filter · Space pause · Q quit".to_string();
        if let Some(t) = app.health.paused {
            footer.push_str(&format!(" · paused since {}", text::stamp_seconds(t)));
        }
        ui.label(RichText::new(footer).weak());
    });
}
