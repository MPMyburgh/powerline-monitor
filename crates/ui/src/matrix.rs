use std::sync::Arc;

use eframe::egui::text::TextWrapping;
use eframe::egui::{self, Align, Align2, Color32, FontId, FontSelection, Galley, RichText, Sense, Stroke, StrokeKind, Ui, WidgetText};
use powerline_core::config::Config;
use powerline_core::history::Ring;
use powerline_core::model::{Mac, NetState, Role};
use powerline_core::standard::signal_level_text;
use powerline_core::text;

use crate::poller::Shared;
use crate::{health, theme, widgets, App, Tab};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Class {
    Good,
    Reduced,
    Poor,
}

impl Class {
    pub fn fill(self) -> Color32 {
        match self {
            Class::Good => theme::GOOD,
            Class::Reduced => theme::REDUCED,
            Class::Poor => theme::POOR,
        }
    }

    pub fn cue(self) -> &'static str {
        match self {
            Class::Good => "",
            Class::Reduced => " ↓",
            Class::Poor => " !",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Class::Good => "Normal for this link",
            Class::Reduced => "Below usual",
            Class::Poor => "Poor",
        }
    }
}

/// Poor under the floor or far below the ring median (needs 10 polls); Reduced under 80 % of the median.
pub fn classify(cfg: &Config, ring: &Ring, from: Mac, to: Mac, now: u16) -> Class {
    let median = ring.median(from, to).map(u32::from);
    let now = u32::from(now);
    if now < u32::from(cfg.alerts.floor_mbps)
        || median.is_some_and(|m| ring.len() >= 10 && now * 100 < m * (100 - u32::from(cfg.alerts.drop_pct)))
    {
        Class::Poor
    } else if median.is_some_and(|m| now * 100 < m * 80) {
        Class::Reduced
    } else {
        Class::Good
    }
}

pub fn show(ui: &mut Ui, app: &mut App, s: &Shared) {
    let Some(st) = &s.state else {
        ui.label("waiting for the first poll");
        return;
    };
    let macs = st.macs();
    if macs.len() < 2 {
        ui.label(if macs.is_empty() {
            "No adapter answered."
        } else {
            "Only one adapter answered; a link needs two. The others may be unplugged or on a different network password."
        });
        return;
    }
    let cfg = &s.cfg;
    let p = theme::palette(ui.ctx());
    ui.label("Link speed between adapters, Mbit/s. Row = sending adapter, column = receiving.")
        .on_hover_text("This is the PHY rate the adapter reports; real file-transfer speed is roughly 40-60% of it.");
    ui.add_space(6.0);
    let n = macs.len() as f32;
    let cell_w = ((ui.available_width() - 130.0) / n - 6.0).clamp(96.0, 160.0);
    let cell_h = (cell_w * 0.5).clamp(44.0, 64.0);
    let selected = app.selected;
    let both = app.visual.both_dirs;
    let mut clicked = None;
    let mut copy = None;
    egui::Grid::new("matrix").spacing([6.0, 6.0]).show(ui, |ui| {
        ui.allocate_exact_size(egui::vec2(110.0, 0.0), Sense::hover());
        for m in &macs {
            header(ui, cfg, *m, selected.is_some_and(|(_, t)| t == *m), &p, cell_w, Align::Center);
        }
        ui.end_row();
        for (i, from) in macs.iter().enumerate() {
            header(ui, cfg, *from, selected.is_some_and(|(f, _)| f == *from), &p, 110.0, Align::Max);
            for (j, to) in macs.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), Sense::click());
                if from == to {
                    ui.painter().rect_filled(rect, 4.0, p.tile);
                    let role = st.snapshot(*from).map_or(Role::Unknown, |s| s.role);
                    let (fill, fg) = theme::role_colours(role, &p);
                    let g = galley(ui, RichText::new(role.human()).size(11.0).strong().color(fg), f32::INFINITY, Align::Center);
                    let badge = egui::Rect::from_center_size(rect.center(), egui::vec2(g.size().x.max(32.0) + 12.0, g.size().y + 4.0));
                    ui.painter().rect_filled(badge, 3.0, fill);
                    ui.painter().galley(egui::pos2(rect.center().x, badge.top() + 2.0), g, fg);
                    if let Some(hint) = widgets::role_hint(role) {
                        resp.on_hover_text(hint);
                    }
                    continue;
                }
                if both && i > j {
                    ui.painter().rect_filled(rect, 4.0, p.tile);
                    continue;
                }
                let cell = cell(s, st, *from, *to, both, &p);
                let painter = ui.painter();
                painter.rect_filled(rect, 4.0, cell.fill);
                let stroke = if selected == Some((*from, *to)) { Some(Stroke::new(2.0, p.accent)) } else { cell.stroke };
                if let Some(stroke) = stroke {
                    painter.rect_stroke(rect, 4.0, stroke, StrokeKind::Inside);
                }
                let size = if both { 16.0 } else { 18.0 };
                match &cell.second {
                    Some(second) => {
                        painter.text(rect.center() - egui::vec2(0.0, 7.0), Align2::CENTER_CENTER, &cell.main, FontId::proportional(size), cell.fg);
                        painter.text(rect.center() + egui::vec2(0.0, 12.0), Align2::CENTER_CENTER, second, FontId::proportional(11.0), cell.fg);
                    }
                    None => {
                        painter.text(rect.center(), Align2::CENTER_CENTER, &cell.main, FontId::proportional(size), cell.fg);
                    }
                }
                let resp = resp.on_hover_text(cell.tooltip);
                if resp.clicked() {
                    clicked = Some((*from, *to));
                }
                if let Some(json) = cell.json {
                    resp.context_menu(|ui| {
                        if ui.button("Copy link JSON").clicked() {
                            copy = Some(json.clone());
                            ui.close();
                        }
                    });
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "Good · Reduced ↓ (below usual) · Poor ! (under {} Mbit/s or far below usual)",
            cfg.alerts.floor_mbps
        ))
        .weak(),
    );
    ui.checkbox(&mut app.visual.both_dirs, "Both directions in one cell");
    if let Some(text) = copy {
        health::copy(ui.ctx(), app, text);
    }
    if let Some(link) = clicked {
        app.selected = Some(link);
        app.visual.auto = None;
        app.tab = Tab::History;
    }
}

/// Name over short MAC, painted into a `width`-wide cell so the grid row gets the real two-line height.
fn header(ui: &mut Ui, cfg: &Config, mac: Mac, accent: bool, p: &theme::Palette, width: f32, halign: Align) {
    let mut name = RichText::new(cfg.name(mac)).size(13.0).strong();
    if accent {
        name = name.color(p.accent);
    }
    let name = galley(ui, name, width, halign);
    let mac = galley(ui, widgets::mac_short(mac), width, halign);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, name.size().y + mac.size().y), Sense::hover());
    let x = match halign {
        Align::Min => rect.left(),
        Align::Center => rect.center().x,
        Align::Max => rect.right(),
    };
    let y = rect.top() + name.size().y;
    ui.painter().galley(egui::pos2(x, rect.top()), name, p.text);
    ui.painter().galley(egui::pos2(x, y), mac, p.muted);
}

fn galley(ui: &Ui, text: RichText, width: f32, halign: Align) -> Arc<Galley> {
    let mut job = Arc::unwrap_or_clone(WidgetText::from(text).into_layout_job(ui.style(), FontSelection::Default, Align::Center));
    job.halign = halign;
    job.wrap = TextWrapping::truncate_at_width(width);
    ui.painter().layout_job(job)
}

struct Cell {
    main: String,
    second: Option<String>,
    fill: Color32,
    fg: Color32,
    stroke: Option<Stroke>,
    tooltip: String,
    json: Option<String>,
}

fn cell(s: &Shared, st: &NetState, from: Mac, to: Mac, both: bool, p: &theme::Palette) -> Cell {
    let cfg = &s.cfg;
    let (fname, tname) = (cfg.name(from), cfg.name(to));
    match st.rate(from, to) {
        Some((tx, rx_own)) => {
            let class = classify(cfg, &s.ring, from, to, tx);
            let rx_peer = st.rate(to, from).map(|r| r.1);
            let median = s.ring.median(from, to);
            let mut tip = format!("{fname} → {tname}: {tx} Mbit/s now");
            if let Some(m) = median {
                tip.push_str(&format!(", usually {m}"));
            }
            tip.push_str(&format!(". {}\n{fname} reports tx {tx}", class.word()));
            if let Some(rx) = rx_peer {
                tip.push_str(&format!(" · {tname} reports rx {rx}"));
            }
            if let Some(h) = st.snapshot(from).and_then(|sn| sn.heard.iter().find(|h| h.mac == to)) {
                tip.push_str(&format!("\nsignal {} · BLE {}", signal_level_text(h.signal_level), h.avg_ble));
            }
            if let Some(l) = st.snapshot(from).and_then(|sn| sn.links.iter().find(|l| l.peer == to)) {
                tip.push_str(&format!("\n{}", l.tx_flags.label()));
            }
            let (main, class) = if both {
                let worst = class.max(classify(cfg, &s.ring, to, from, rx_own));
                (format!("{tx} / {rx_own}{}", worst.cue()), worst)
            } else {
                (format!("{tx}{}", class.cue()), class)
            };
            let second = (!both)
                .then_some(rx_peer)
                .flatten()
                .filter(|rx| u32::from(rx.abs_diff(tx)) * 20 > u32::from(tx))
                .map(|rx| format!("rx {rx}"));
            let json = serde_json::json!({
                "from": from, "to": to, "from_name": fname, "to_name": tname,
                "tx_mbps": tx, "rx_mbps": rx_own, "at": text::stamp_full(st.at),
            });
            Cell { main, second, fill: class.fill(), fg: Color32::BLACK, stroke: None, tooltip: tip, json: Some(json.to_string()) }
        }
        None => match st.rate(to, from) {
            Some((_, rx)) => Cell {
                main: format!("{rx}~"),
                second: None,
                fill: p.tile,
                fg: p.text,
                stroke: Some(Stroke::new(1.5, p.accent)),
                tooltip: format!("{fname} did not answer; this is {tname}'s figure for what it receives"),
                json: None,
            },
            None => Cell {
                main: "no data".to_string(),
                second: None,
                fill: theme::NODATA,
                fg: theme::NODATA_TEXT,
                stroke: None,
                tooltip: format!("Neither {fname} nor {tname} reported a rate for this link"),
                json: None,
            },
        },
    }
}
