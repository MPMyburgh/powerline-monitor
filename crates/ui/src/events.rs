use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, RichText, Sense, Theme, Ui};
use powerline_core::events::{Event, Severity};
use powerline_core::model::Mac;
use powerline_core::text;

use crate::poller::Shared;
use crate::theme::Palette;
use crate::{health, theme, widgets, App, Tab};

enum Line<'a> {
    Day(String),
    Event(&'a Event),
}

pub fn show(ui: &mut Ui, app: &mut App, s: &Shared) {
    let p = theme::palette(ui.ctx());
    let cfg = &s.cfg;
    let name = |m: Mac| cfg.name(m);
    let mut copy = false;
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut app.health.min_severity,
            &[("All", Severity::Info), ("Warnings", Severity::Warning), ("Critical", Severity::Critical)],
        );
        let r = ui.add(egui::TextEdit::singleline(&mut app.health.filter).hint_text("filter (/)").desired_width(220.0));
        if std::mem::take(&mut app.health.focus_filter) {
            r.request_focus();
        }
        copy = ui.button("Copy as text").clicked();
    });
    let filter = app.health.filter.trim().to_lowercase();
    let min = app.health.min_severity;
    let rows: Vec<&Event> = s
        .events
        .iter()
        .rev()
        .filter(|e| {
            e.severity() >= min
                && (filter.is_empty() || e.summary(&name).to_lowercase().contains(&filter) || e.kind().contains(&filter))
        })
        .collect();
    if copy {
        let lines: Vec<String> =
            rows.iter().map(|e| format!("{}  {}  {}", text::stamp_full(e.at()), e.kind(), e.summary(&name))).collect();
        health::copy(ui.ctx(), app, lines.join("\n"));
    }
    if s.events.is_empty() {
        ui.label(
            RichText::new(
                "Nothing has changed since the window opened. Adapters going offline, the coordinator changing or a link slowing down will appear here.",
            )
            .weak(),
        );
        return;
    }
    if rows.is_empty() {
        ui.label(RichText::new("No events match the filter.").weak());
        return;
    }
    let mut lines = Vec::new();
    let mut day = None;
    for e in rows {
        let d = text::day_label(e.at());
        if day.as_ref() != Some(&d) {
            lines.push(Line::Day(d.clone()));
            day = Some(d);
        }
        lines.push(Line::Event(e));
    }
    let tint = if ui.ctx().theme() == Theme::Dark { theme::CRITICAL_ROW_DARK } else { theme::CRITICAL_ROW_LIGHT };
    let tints: Vec<Option<Color32>> =
        lines.iter().map(|l| matches!(l, Line::Event(e) if e.severity() == Severity::Critical).then_some(tint)).collect();
    let summary_width = (ui.available_width() - 300.0).max(240.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("events")
            .spacing([12.0, 6.0])
            .with_row_color(move |i, _| tints.get(i).copied().flatten())
            .show(ui, |ui| {
                for line in &lines {
                    match line {
                        Line::Day(d) => {
                            ui.vertical(|ui| {
                                ui.add_space(8.0);
                                ui.label(RichText::new(d.to_uppercase()).size(11.0).color(p.muted));
                            });
                        }
                        Line::Event(e) => row(ui, app, e, &name, &p, summary_width),
                    }
                    ui.end_row();
                }
            });
    });
}

fn row(ui: &mut Ui, app: &mut App, e: &Event, name: &dyn Fn(Mac) -> String, p: &Palette, width: f32) {
    ui.label(RichText::new(clock(e.at())).monospace().size(12.0).color(p.muted));
    let (chip, (fill, fg)) = match e.severity() {
        Severity::Info => ("● info", theme::INFO_CHIP),
        Severity::Warning => ("▲ warning", theme::WARNING_CHIP),
        Severity::Critical => ("■ critical", theme::CRITICAL_CHIP),
    };
    ui.scope(|ui| {
        ui.set_min_width(56.0);
        widgets::badge(ui, chip, fill, fg);
    });
    let link = match e {
        Event::RateDegraded { from, to, .. } | Event::RateRecovered { from, to, .. } => Some((*from, *to)),
        _ => None,
    };
    let r = ui
        .vertical(|ui| {
            ui.set_width(width);
            let mut label = egui::Label::new(e.summary(name));
            if link.is_some() {
                label = label.sense(Sense::click());
            }
            let r = ui.add(label);
            if let Some(advice) = e.advice() {
                ui.label(RichText::new(advice).size(12.0).color(p.muted));
            }
            r
        })
        .inner;
    if let Some(l) = link {
        if r.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            app.selected = Some(l);
            app.tab = Tab::History;
        }
    }
    ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
        ui.label(RichText::new(text::ago(e.at())).size(11.0).weak());
    });
}

fn clock(t: SystemTime) -> String {
    let tm = text::local_tm(t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64));
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
