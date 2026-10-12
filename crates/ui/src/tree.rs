use eframe::egui::{self, RichText, Sense, Ui};
use powerline_core::config::Config;
use powerline_core::keys;
use powerline_core::model::{Mac, NetState, Nid, Role, Snapshot};
use powerline_core::text;

use crate::poller::Shared;
use crate::{health, settings, theme, widgets, App, Tab};

pub fn tree(ui: &mut Ui, app: &mut App, s: &Shared) {
    let Some(st) = &s.state else {
        ui.label(RichText::new("waiting for the first poll").weak());
        return;
    };
    let cfg = &s.cfg;
    let p = theme::palette(ui.ctx());
    let mut ifaces: Vec<&str> = st.devices.iter().map(|d| d.iface.as_str()).collect();
    ifaces.sort();
    ifaces.dedup();
    let single = ifaces.len() == 1;
    for iface in ifaces {
        if single {
            port(ui, app, st, cfg, iface);
        } else {
            egui::CollapsingHeader::new(RichText::new(format!("Port {iface}")).strong())
                .default_open(true)
                .show(ui, |ui| port(ui, app, st, cfg, iface));
        }
    }
    for (mac, since) in app.health.not_answering(st) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(cfg.name(mac)).size(14.0).color(p.muted));
            ui.label(
                RichText::new(format!("not answering since {} ({})", text::stamp(since), text::ago(since))).size(11.0).color(p.muted),
            );
        });
    }
    ui.add_space(8.0);
    ui.checkbox(&mut app.health.show_details, "Show technical details");
}

fn port(ui: &mut Ui, app: &mut App, st: &NetState, cfg: &Config, iface: &str) {
    let p = theme::palette(ui.ctx());
    let macs: Vec<Mac> = st.macs().into_iter().filter(|m| st.devices.iter().any(|d| d.mac == *m && d.iface == iface)).collect();
    let mut nids: Vec<Nid> = macs.iter().filter_map(|m| st.snapshot(*m)).map(|s| s.nid).collect();
    nids.sort();
    nids.dedup();
    for nid in nids {
        let mut members: Vec<&Snapshot> = macs.iter().filter_map(|m| st.snapshot(*m)).filter(|s| s.nid == nid).collect();
        members.sort_by_key(|s| (s.role, s.device));
        let who = |m: Option<Mac>| m.map_or_else(|| "unknown".to_string(), |m| cfg.name(m));
        let mut title = format!(
            "Powerline network · {} · coordinator {}",
            text::plural(members.len(), "adapter", "adapters"),
            who(members.iter().find_map(|s| s.cco))
        );
        if let Some(b) = members.iter().find_map(|s| s.bcco) {
            title.push_str(&format!(" · backup {}", cfg.name(b)));
        }
        let mut heard: Vec<Nid> = members.iter().flat_map(|s| s.neighbour_nets.iter()).map(|n| n.nid).filter(|n| *n != nid).collect();
        heard.sort();
        heard.dedup();
        let mut clicked = false;
        let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(nid.to_string()), true)
            .show_header(ui, |ui| {
                let r = ui
                    .add(egui::Label::new(title).wrap().sense(Sense::click()))
                    .on_hover_text(format!("NID {nid} · SNID {} · security level {}", members[0].snid, nid.security_level()));
                clicked = r.clicked();
            });
        if clicked {
            header.toggle();
        }
        header.body(|ui| {
            if !heard.is_empty() {
                let colour = if heard.iter().any(keys::is_default_nid) { p.crit } else { p.warn };
                ui.label(RichText::new(format!("Other networks heard on the wiring: {}", heard.len())).color(colour));
            }
            for s in &members {
                adapter_row(ui, app, st, cfg, s.device, Some(s));
            }
        });
    }
    for m in macs.iter().filter(|m| st.snapshot(**m).is_none()) {
        adapter_row(ui, app, st, cfg, *m, None);
    }
}

fn adapter_row(ui: &mut Ui, app: &mut App, st: &NetState, cfg: &Config, mac: Mac, snap: Option<&Snapshot>) {
    let d = st.devices.iter().find(|d| d.mac == mac).unwrap();
    let role = snap.map_or(Role::Unknown, |s| s.role);
    let p = theme::palette(ui.ctx());
    let name = cfg.name(mac);
    let mut r = ui
        .horizontal_wrapped(|ui| {
            widgets::role_badge(ui, role, &p);
            let r = settings::inline_name(ui, app, cfg, mac);
            if d.local {
                widgets::badge(ui, "wired here", theme::WIRED_FILL, theme::WIRED_FG)
                    .on_hover_text("This adapter is connected to this computer's Ethernet port");
            }
            if d.seen_by_peers_only {
                widgets::badge(ui, "seen by peers, not answering", p.tile, p.warn);
            }
            r
        })
        .inner;
    let uptime = snap.and_then(|s| s.uptime_secs).map(|u| format!("up {}", text::duration(u.into())));
    let detail = if app.health.show_details {
        let mut parts = vec![format!("MAC {mac}")];
        if let Some(s) = snap {
            parts.push(format!("TEI {}", s.tei));
        }
        match snap.and_then(|s| s.firmware.as_deref()) {
            Some(fw) => parts.push(format!("firmware {fw}")),
            None => {
                if let Some(h) = d.hfid.as_deref().filter(|h| !h.is_empty()) {
                    parts.push(format!("firmware tag {h}"));
                }
            }
        }
        parts.extend(uptime);
        if let Some(av) = d.caps.as_ref().map(|c| c.av_version).or_else(|| d.ident.as_ref().map(|i| i.hpav)) {
            let mimo = match d.ident.as_ref().map(|i| i.mcs) {
                Some(0) => " SISO",
                Some(_) => " MIMO",
                None => "",
            };
            parts.push(format!("AV{}{mimo}", if av == 0 { "1.1" } else { "2.0" }));
        }
        Some(parts.join(" · "))
    } else {
        uptime.filter(|_| d.local)
    };
    if let Some(line) = detail {
        ui.indent(mac, |ui| {
            r = r.clone() | ui.add(egui::Label::new(RichText::new(line).size(11.0).weak()).sense(Sense::click()).selectable(false));
        });
    }
    r.context_menu(|ui| {
        if ui.button("Copy MAC").clicked() {
            health::copy(ui.ctx(), app, mac.to_string());
            ui.close();
        }
        if ui.button("Copy name").clicked() {
            health::copy(ui.ctx(), app, name.clone());
            ui.close();
        }
        if let Some(s) = snap {
            if ui.button("Copy adapter JSON").clicked() {
                health::copy(ui.ctx(), app, serde_json::to_string_pretty(s).unwrap_or_default());
                ui.close();
            }
        }
    });
    if r.double_clicked() {
        app.health.selected_adapter = Some(mac);
        app.tab = Tab::Adapter;
    }
}
