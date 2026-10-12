use std::path::Path;

use eframe::egui::{self, Color32, CornerRadius, Margin, Rect, RichText, Theme, Ui};
use powerline_core::config::Config;
use powerline_core::events::Severity;
use powerline_core::model::NetState;
use powerline_core::render;
use powerline_core::text;

use crate::poller::Shared;
use crate::{health, overview, theme, widgets, App, Tab};

pub fn show_top(ui: &mut Ui, app: &mut App, s: &Shared) {
    verdict_strip(ui, app, s);
    if let Some(st) = s.state.as_ref().filter(|st| st.default_key()) {
        security_strip(ui, app, s, st);
    }
    if let Some(e) = &s.error {
        error_card(ui, app, s, e);
        if let Some(st) = s.state.as_ref().filter(|st| !st.devices.is_empty()) {
            let p = theme::palette(ui.ctx());
            ui.label(
                RichText::new(format!("Showing the last good reading from {} ({})", text::stamp(st.at), text::ago(st.at)))
                    .color(p.warn),
            );
            ui.visuals_mut().override_text_color = Some(p.muted);
        }
    }
    health::draw_toast(ui.ctx(), app);
}

pub fn verdict_strip(ui: &mut Ui, app: &mut App, s: &Shared) {
    let Some(v) = overview::verdict(ui.ctx(), s) else { return };
    let p = theme::palette(ui.ctx());
    let (pill, colour) = match v.severity {
        Some(Severity::Critical) => ("PROBLEM", p.crit),
        Some(Severity::Warning) => ("ATTENTION", p.warn),
        _ => ("OK", p.ok),
    };
    let sentence = v.line.split_once("  ").map_or(v.line.as_str(), |(_, rest)| rest);
    strip(ui, p.tile, colour, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(st) = s.state.as_ref().filter(|st| !st.devices.is_empty()) {
                    if ui.button("Copy summary").clicked() {
                        health::copy(ui.ctx(), app, render::status_text(st, &s.cfg, Some(&s.tracker), false));
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    widgets::badge(ui, pill, colour, p.panel).on_hover_text(format!(
                        "Decided from the last poll: adapters missing, unknown adapters, links under {} Mbit/s or far below usual, then the network password.",
                        s.cfg.alerts.floor_mbps
                    ));
                    ui.add(egui::Label::new(RichText::new(sentence).size(14.0).strong()).wrap());
                });
            });
        });
        let newest = s.events.iter().rev().find(|e| e.severity() >= Severity::Warning);
        let line = match newest {
            Some(e) => format!("{}  {}", text::stamp(e.at()), e.summary(&|m| s.cfg.name(m))),
            None => "Nothing has changed since the window opened".to_string(),
        };
        ui.label(RichText::new(line).color(p.muted));
    });
}

fn security_strip(ui: &mut Ui, app: &mut App, s: &Shared, st: &NetState) {
    let p = theme::palette(ui.ctx());
    let fill = if ui.ctx().theme() == Theme::Dark { theme::KEY_FILL_DARK } else { theme::KEY_FILL_LIGHT };
    strip(ui, fill, p.warn, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("🔒 Factory password still in use").size(13.0).strong());
            ui.toggle_value(&mut app.health.explain_key, "What does this mean");
            if ui.link("Events").clicked() {
                app.tab = Tab::Events;
            }
        });
        if app.health.explain_key {
            ui.label(
                "Your powerline network still has the factory password HomePlugAV. Any HomePlug adapter plugged into this wiring, \
                 a neighbour's included, joins your home network without pairing. powerline-monitor never changes keys from this window; \
                 fix it in a terminal:",
            );
            let cmd = rekey_command(&s.cfg, st, &app.config_path);
            ui.label(RichText::new(&cmd).monospace());
            if ui.button("Copy command").clicked() {
                health::copy(ui.ctx(), app, cmd);
            }
        }
    });
}

fn rekey_command(cfg: &Config, st: &NetState, config_path: &Path) -> String {
    let iface = st
        .devices
        .iter()
        .find(|d| d.local)
        .map(|d| d.iface.clone())
        .unwrap_or_else(|| cfg.effective_interfaces().into_iter().next().unwrap_or_default());
    if cfg.devices.is_empty() {
        format!(
            "1. Name your adapters below (Settings → Names) and Save, or run: powerline-monitor discover --toml >> {}\n\
             2. powerline-monitor -i {iface} rekey --dry-run, then the same without --dry-run",
            config_path.display()
        )
    } else {
        format!("powerline-monitor -i {iface} rekey --dry-run")
    }
}

fn error_card(ui: &mut Ui, app: &mut App, s: &Shared, error: &str) {
    let p = theme::palette(ui.ctx());
    let mut copy = None;
    let mut settings = false;
    let cause = if error.contains("no adapters found") {
        format!(
            "No powerline adapter answered on {}. Is this computer plugged into one of the adapters with an Ethernet cable, and is its Power light on?",
            s.cfg.effective_interfaces().join(", ")
        )
    } else if error.contains("CAP_NET_RAW") {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "<binary>".to_string());
        let cmd = format!("sudo setcap cap_net_raw+ep {exe}");
        let cause = format!("powerline-monitor needs permission to talk to the adapters. In a terminal run: {cmd} (or re-run install.sh), then Retry");
        copy = Some(cmd);
        cause
    } else if error.contains("not found") {
        settings = true;
        let name = error.split(" not found").next().and_then(|a| a.rsplit(' ').next()).unwrap_or("?");
        format!("There is no network port called {name}")
    } else if error.contains("no ethernet interface is up") {
        "No wired network port is connected. Plug this computer into a powerline adapter.".to_string()
    } else {
        error.to_string()
    };
    strip(ui, theme::ERROR_FILL, p.crit, |ui| {
        ui.add(egui::Label::new(RichText::new(cause).color(Color32::WHITE)).wrap()).on_hover_text(error);
        ui.horizontal(|ui| {
            if ui.button("Retry").clicked() {
                app.refresh.send(()).ok();
            }
            if let Some(cmd) = copy {
                if ui.button("Copy").clicked() {
                    health::copy(ui.ctx(), app, cmd);
                }
            }
            if settings && ui.button("Open Settings").clicked() {
                app.tab = Tab::Settings;
            }
        });
    });
}

/// A full-width card with a 3 px accent along its left edge.
fn strip(ui: &mut Ui, fill: Color32, edge: Color32, add: impl FnOnce(&mut Ui)) {
    let radius = CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 };
    let r = egui::Frame::new().fill(fill).corner_radius(radius).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    let rect = r.response.rect;
    ui.painter().rect_filled(Rect::from_min_max(rect.left_top(), egui::pos2(rect.left() + 3.0, rect.bottom())), 0.0, edge);
}
