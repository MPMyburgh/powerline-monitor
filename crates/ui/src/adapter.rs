use std::fs;

use eframe::egui::{self, Color32, RichText, Sense, Shape, Stroke, Ui};
use powerline_core::model::{Device, Mac, Vendor};
use powerline_core::standard::signal_level_text;
use powerline_core::text;

use crate::poller::Shared;
use crate::settings::{self, label};
use crate::{health, theme, widgets, App};

const ASYMMETRY_TIP: &str = "All links from one adapter slow = its socket; one direction slow = the path";

pub fn show(ui: &mut Ui, app: &mut App, s: &mut Shared) {
    settings::flush(app, s);
    if let Some(arg) = s.adapter_arg.take() {
        match &s.state {
            Some(st) => match s.cfg.resolve_adapter(&arg, &st.macs()) {
                Ok(m) => app.health.selected_adapter = Some(m),
                Err(e) => eprintln!("--adapter: {e}"),
            },
            None => s.adapter_arg = Some(arg),
        }
    }
    let Some(mac) = app.health.selected_adapter else {
        ui.label("Pick an adapter in the list on the left, or press 6 after selecting one");
        return;
    };
    let Some(st) = &s.state else {
        ui.label(RichText::new("waiting for the first poll").weak());
        return;
    };
    let p = theme::palette(ui.ctx());
    let cfg = &s.cfg;
    let name = |m: Mac| cfg.name(m);
    let dev = st.devices.iter().find(|d| d.mac == mac);
    let snap = st.snapshot(mac);
    let value_width = (ui.available_width() - 180.0).max(240.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("facts").min_col_width(140.0).spacing([12.0, 6.0]).show(ui, |ui| {
            label(ui, "Name");
            ui.horizontal(|ui| {
                settings::inline_name(ui, app, cfg, mac);
            });
            ui.end_row();
            label(ui, "MAC");
            ui.horizontal(|ui| {
                ui.label(widgets::mac(mac));
                if ui.small_button("Copy").clicked() {
                    health::copy(ui.ctx(), app, mac.to_string());
                }
            });
            ui.end_row();
            if let Some(d) = dev {
                label(ui, "Where");
                ui.label(if d.local { "wired to this computer" } else { "over the powerline" });
                ui.end_row();
            }
            label(ui, "Role");
            ui.label(snap.map_or("not answering", |sn| sn.role.human()));
            ui.end_row();
            label(ui, "Firmware");
            let hfid = dev.and_then(|d| d.hfid.as_deref()).filter(|h| !h.is_empty());
            ui.label(match (snap.and_then(|sn| sn.firmware.as_deref()), hfid) {
                (Some(fw), _) => fw.to_string(),
                (None, Some(h)) => format!("firmware tag {h}"),
                (None, None) => "not reported".to_string(),
            });
            ui.end_row();
            label(ui, "Powered on since");
            ui.label(match snap.and_then(|sn| sn.powered_on_since.zip(sn.uptime_secs)) {
                Some((t, up)) => format!("{} · up {}", text::stamp(t), text::duration(up.into())),
                None => "not reported".to_string(),
            });
            ui.end_row();
            if let Some(sn) = snap {
                label(ui, "Network");
                let cco = sn.cco.map_or_else(|| "unknown".to_string(), name);
                ui.label(format!("NID {} · SNID {} · coordinator {cco}", sn.nid, sn.snid));
                ui.end_row();
            }
            label(ui, "Capabilities");
            ui.vertical(|ui| {
                ui.set_max_width(value_width);
                ui.label(dev.map_or_else(|| "not reported".to_string(), capabilities));
            });
            ui.end_row();
            for (what, when) in [("First seen", app.health.first_seen.get(&mac)), ("Last seen", app.health.last_seen.get(&mac))] {
                if let Some(t) = when {
                    label(ui, what);
                    ui.label(format!("{} ({})", text::stamp(*t), text::ago(*t)));
                    ui.end_row();
                }
            }
            if let Some(sn) = snap.filter(|sn| !sn.heard.is_empty()) {
                label(ui, "Signal to peers");
                ui.vertical(|ui| {
                    for h in &sn.heard {
                        ui.label(format!("{}: {} · BLE {}", name(h.mac), signal_level_text(h.signal_level), h.avg_ble));
                    }
                });
                ui.end_row();
            }
        });
        let Some(sn) = snap else { return };
        ui.add_space(10.0);
        let host: Option<Mac> = dev
            .and_then(|d| fs::read_to_string(format!("/sys/class/net/{}/address", d.iface)).ok())
            .and_then(|a| a.trim().parse().ok());
        egui::CollapsingHeader::new(format!("Devices behind this adapter ({})", sn.bridged.len())).default_open(true).show(ui, |ui| {
            if sn.bridged.is_empty() {
                ui.label(RichText::new("none reported").weak());
            }
            for m in &sn.bridged {
                ui.horizontal(|ui| {
                    ui.label(widgets::mac(*m));
                    if Some(*m) == host {
                        widgets::badge(ui, "this computer", theme::WIRED_FILL, theme::WIRED_FG);
                    }
                    if m.0[0] & 0x02 != 0 {
                        ui.label(RichText::new("private/random").weak());
                    }
                });
            }
        });
        if sn.links.is_empty() {
            return;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Links").strong());
        egui::Grid::new("links").spacing([14.0, 6.0]).show(ui, |ui| {
            for h in ["Peer", "Sends", "Receives", "Asymmetry", "Lowest / usual", "Last 60 readings"] {
                ui.label(RichText::new(h).size(11.0).color(p.muted));
            }
            ui.end_row();
            let figure = |v: Option<u16>| v.map_or("?".to_string(), |v| v.to_string());
            for l in &sn.links {
                ui.label(name(l.peer));
                ui.label(RichText::new(format!("{} Mbit/s", l.tx_mbps)).monospace());
                ui.label(RichText::new(format!("{} Mbit/s", l.rx_mbps)).monospace());
                let (tx, rx) = (i32::from(l.tx_mbps), i32::from(l.rx_mbps));
                let pct = (tx - rx) * 100 / tx.max(rx).max(1);
                let mut asym = RichText::new(format!("{:+} ({pct:+}%)", tx - rx)).monospace();
                if pct.abs() > 25 {
                    asym = asym.color(p.warn);
                }
                ui.label(asym).on_hover_text(ASYMMETRY_TIP);
                let (lowest, usual) = (s.ring.window_min(mac, l.peer, 60), s.ring.median(mac, l.peer));
                ui.label(RichText::new(format!("{} / {}", figure(lowest), figure(usual))).monospace());
                let mut series: Vec<u16> = s.ring.series(mac, l.peer).iter().rev().take(60).map(|x| x.1).collect();
                series.reverse();
                sparkline(ui, &series, p.accent);
                ui.end_row();
            }
        });
    });
}

/// Capability summary from CM_STA_CAP / CM_STA_IDENTIFY, e.g. 'HomePlug AV 2.0 · SISO (no MIMO) · 1.8-30.0 MHz · OUI 00:1f:84 (Broadcom)'.
fn capabilities(d: &Device) -> String {
    let mut parts = Vec::new();
    if let Some(av) = d.caps.as_ref().map(|c| c.av_version).or_else(|| d.ident.as_ref().map(|i| i.hpav)) {
        parts.push(format!("HomePlug AV {}", if av == 0 { "1.1" } else { "2.0" }));
    }
    if let Some(i) = &d.ident {
        parts.push((if i.mcs == 0 { "SISO (no MIMO)" } else { "MIMO" }).to_string());
        let (lo, hi) = i.band_mhz();
        if hi > 0.0 {
            parts.push(format!("{lo:.1}-{hi:.1} MHz"));
        }
    }
    if let Some(c) = &d.caps {
        if c.cco_capable {
            parts.push("CCo-capable".to_string());
        }
        if c.backup_cco_capable {
            parts.push("backup-CCo-capable".to_string());
        }
    }
    if d.ident.as_ref().is_some_and(|i| i.power_save_capable) {
        parts.push("power-save capable".to_string());
    }
    if let Some(c) = &d.caps {
        let vendor = match d.vendor {
            Vendor::Broadcom => "Broadcom",
            Vendor::Qualcomm => "Qualcomm",
            Vendor::Unknown => "unknown",
        };
        parts.push(format!("OUI {:02x}:{:02x}:{:02x} ({vendor})", c.oui[0], c.oui[1], c.oui[2]));
    }
    if parts.is_empty() {
        "not reported".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Paints the series as a line scaled to 0..600 Mbit/s in a 120 x 20 px cell.
fn sparkline(ui: &mut Ui, series: &[u16], colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 20.0), Sense::hover());
    if series.len() < 2 {
        return;
    }
    let step = rect.width() / (series.len() - 1) as f32;
    let points = series
        .iter()
        .enumerate()
        .map(|(i, v)| egui::pos2(rect.left() + step * i as f32, rect.bottom() - rect.height() * (f32::from(*v) / 600.0).min(1.0)))
        .collect();
    ui.painter().add(Shape::line(points, Stroke::new(1.5, colour)));
}
