use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use eframe::egui::{self, Align, Key, Layout, RichText, Sense, Ui};
use powerline_core::config::{Alerts, Config};
use powerline_core::model::{Mac, NetState, Role};
use powerline_core::text;

use crate::poller::Shared;
use crate::{theme, widgets, App};

#[derive(Clone, PartialEq)]
struct Values {
    interfaces: Vec<String>,
    poll_seconds: u64,
    history_hours: u64,
    names: BTreeMap<Mac, String>,
    spares: BTreeMap<Mac, String>,
    alerts: Alerts,
}

impl Values {
    fn new(cfg: &Config) -> Values {
        Values {
            interfaces: cfg.interfaces.clone(),
            poll_seconds: cfg.poll_seconds,
            history_hours: cfg.history_hours,
            names: cfg.devices.iter().map(|(m, d)| (*m, d.alias.clone().unwrap_or_default())).collect(),
            spares: cfg.spares.clone(),
            alerts: cfg.alerts,
        }
    }

    fn apply(&self, base: &Config) -> Config {
        let mut cfg = base.clone();
        cfg.interfaces = self.interfaces.clone();
        cfg.poll_seconds = self.poll_seconds;
        cfg.history_hours = self.history_hours;
        cfg.alerts = self.alerts;
        for (mac, name) in &self.names {
            let name = name.trim();
            cfg.devices.entry(*mac).or_default().alias = (!name.is_empty()).then(|| name.to_string());
        }
        cfg.devices.retain(|m, d| d.alias.is_some() || d.vendor.is_some() || base.devices.contains_key(m));
        cfg.spares = self.spares.iter().map(|(m, n)| (*m, n.trim().to_string())).collect();
        cfg
    }
}

pub struct Form {
    v: Values,
    new_spare: String,
    pending_rename: Option<(Mac, String)>,
    pending_save: bool,
    msg: Option<(String, bool, Instant)>,
}

impl Form {
    pub fn new(cfg: &Config) -> Form {
        Form { v: Values::new(cfg), new_spare: String::new(), pending_rename: None, pending_save: false, msg: None }
    }

    fn dirty(&self, cfg: &Config) -> bool {
        Values::new(&self.v.apply(cfg)) != Values::new(cfg)
    }
}

pub fn show(ui: &mut Ui, app: &mut App, s: &mut Shared) {
    flush(app, s);
    let p = theme::palette(ui.ctx());
    let ctx = ui.ctx().clone();
    if app.form.msg.as_ref().is_some_and(|(_, ok, at)| *ok && at.elapsed() > Duration::from_secs(3)) {
        app.form.msg = None;
    }
    let seen: Vec<Mac> = s.state.as_ref().map(|st| st.macs()).unwrap_or_default();
    for mac in seen.iter().filter(|m| !s.cfg.is_spare(**m)) {
        app.form.v.names.entry(*mac).or_default();
    }
    let ports = ports(s.state.as_ref(), &app.form.v.interfaces);
    let (mut do_save, mut revert) = (false, false);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let form = &mut app.form;
        let cfg = &s.cfg;
        let st = s.state.as_ref();
        egui::Grid::new("general").min_col_width(140.0).spacing([12.0, 8.0]).show(ui, |ui| {
            label(ui, "Network ports");
            ui.vertical(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                let mut auto = form.v.interfaces.is_empty();
                if ui.checkbox(&mut auto, "Automatic: every connected port").changed() && auto {
                    form.v.interfaces.clear();
                }
                for (name, text) in &ports {
                    let mut on = form.v.interfaces.contains(name);
                    if ui.checkbox(&mut on, text).changed() {
                        if on {
                            form.v.interfaces.push(name.clone());
                        } else {
                            form.v.interfaces.retain(|n| n != name);
                        }
                    }
                }
            });
            ui.end_row();
            label(ui, "Check every");
            ui.add(egui::DragValue::new(&mut form.v.poll_seconds).range(5..=600).suffix(" s"));
            ui.end_row();
            label(ui, "Keep");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut form.v.history_hours).range(1..=720).suffix(" h"));
                ui.label("of history in this window");
            });
            ui.end_row();
        });

        section(ui, "Names");
        ui.label(RichText::new("Name each adapter after the room it is in, e.g. Lounge (router)").weak());
        egui::Grid::new("names").spacing([10.0, 6.0]).show(ui, |ui| {
            for (mac, name) in &mut form.v.names {
                let role = st.and_then(|st| st.snapshot(*mac)).map_or(Role::Unknown, |sn| sn.role);
                widgets::role_badge(ui, role, &p);
                ui.label(widgets::mac(*mac));
                if st.is_some_and(|st| st.devices.iter().any(|d| d.mac == *mac && d.local)) {
                    widgets::badge(ui, "wired here", theme::WIRED_FILL, theme::WIRED_FG);
                } else {
                    ui.label("");
                }
                ui.add_sized([320.0, 22.0], egui::TextEdit::singleline(name).hint_text(cfg.name(*mac)));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Spares").strong());
        let mut remove = None;
        egui::Grid::new("spares").spacing([10.0, 6.0]).show(ui, |ui| {
            for (mac, name) in &mut form.v.spares {
                ui.label(widgets::mac(*mac));
                ui.add_sized([320.0, 22.0], egui::TextEdit::singleline(name));
                if ui.small_button("Remove").clicked() {
                    remove = Some(*mac);
                }
                ui.end_row();
            }
        });
        if let Some(m) = remove {
            form.v.spares.remove(&m);
        }
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut form.new_spare).hint_text("Add spare by MAC").desired_width(180.0));
            let entered = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if (ui.button("Add").clicked() || entered) && !form.new_spare.trim().is_empty() {
                match form.new_spare.trim().parse::<Mac>() {
                    Ok(m) => {
                        form.v.spares.insert(m, format!("Spare {:02X}{:02X}", m.0[4], m.0[5]));
                        form.new_spare.clear();
                    }
                    Err(e) => form.msg = Some((e.to_string(), false, Instant::now())),
                }
            }
        });

        section(ui, "Alerts");
        let a = &mut form.v.alerts;
        egui::Grid::new("alerts").min_col_width(140.0).spacing([12.0, 8.0]).show(ui, |ui| {
            label(ui, "Call a link poor below");
            ui.add(egui::DragValue::new(&mut a.floor_mbps).range(1..=2000).suffix(" Mbit/s"));
            ui.end_row();
            label(ui, "or when it drops");
            ui.add(egui::DragValue::new(&mut a.drop_pct).range(1..=90).suffix(" % below usual"));
            ui.end_row();
            label(ui, "after");
            ui.add(egui::DragValue::new(&mut a.degraded_after_polls).range(1..=100).suffix(" polls"));
            ui.end_row();
            label(ui, "Report an adapter offline after");
            ui.add(egui::DragValue::new(&mut a.lost_after_polls).range(1..=20).suffix(" missed polls"));
            ui.end_row();
            label(ui, "Appearance");
            theme::appearance_row(ui, &ctx);
            ui.end_row();
        });

        ui.add_space(12.0);
        let dirty = form.dirty(&s.file_cfg);
        ui.horizontal(|ui| {
            do_save = ui.add_enabled(dirty, egui::Button::new("Save")).clicked();
            revert = ui.add_enabled(dirty, egui::Button::new("Revert")).clicked();
            if dirty {
                ui.label(RichText::new("Unsaved changes").color(p.warn));
            }
            if let Some((m, ok, _)) = &form.msg {
                ui.label(RichText::new(m).color(if *ok { p.ok } else { p.crit }));
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new(app.config_path.display().to_string()).monospace().size(11.0).weak());
    });
    if revert {
        app.form.v = Values::new(&s.file_cfg);
    }
    if do_save {
        save(app, s);
    }
}

/// The adapter's name with a pencil button; while editing, Enter queues the rename and Escape or clicking away cancels.
pub fn inline_name(ui: &mut Ui, app: &mut App, cfg: &Config, mac: Mac) -> egui::Response {
    if app.health.editing.as_ref().is_some_and(|(m, _)| *m == mac) {
        let r = {
            let (_, text) = app.health.editing.as_mut().unwrap();
            ui.add(egui::TextEdit::singleline(text).hint_text("e.g. Lounge (router)").desired_width(220.0))
        };
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() {
            let (_, text) = app.health.editing.take().unwrap();
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                app.form.pending_rename = Some((mac, text));
            }
        }
        if let Some((m, false, _)) = &app.form.msg {
            ui.label(RichText::new(m).size(11.0).color(theme::palette(ui.ctx()).crit));
        }
        r
    } else {
        let r = ui.add(egui::Label::new(RichText::new(cfg.name(mac)).size(14.0).strong()).sense(Sense::click()));
        if ui.small_button("✏").on_hover_text("Rename").clicked() {
            let current = cfg.devices.get(&mac).and_then(|d| d.alias.clone()).or_else(|| cfg.spares.get(&mac).cloned());
            app.health.editing = Some((mac, current.unwrap_or_default()));
        }
        r
    }
}

/// First-run card for the left panel while a seen adapter has no name; Save queues the form save.
pub fn naming_card(ui: &mut Ui, app: &mut App, s: &Shared) {
    if app.health.dismissed_naming {
        return;
    }
    let Some(st) = &s.state else { return };
    let cfg = &s.cfg;
    let macs: Vec<Mac> = st.macs().into_iter().filter(|m| !cfg.is_spare(*m)).collect();
    if macs.is_empty() || macs.iter().all(|m| cfg.devices.get(m).is_some_and(|d| d.alias.is_some())) {
        return;
    }
    let p = theme::palette(ui.ctx());
    egui::Frame::new().fill(p.tile).corner_radius(6).inner_margin(10).show(ui, |ui| {
        ui.label(
            RichText::new(format!(
                "Give your {} names so this reads like your house, not a MAC table",
                text::plural(macs.len(), "adapter", "adapters")
            ))
            .strong(),
        );
        for mac in &macs {
            let name = app.form.v.names.entry(*mac).or_default();
            ui.add(egui::TextEdit::singleline(name).hint_text(cfg.name(*mac)).desired_width(f32::INFINITY));
            if st.devices.iter().any(|d| d.mac == *mac && d.local) {
                ui.label(RichText::new("(wired to this computer)").size(11.0).weak());
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                app.form.pending_save = true;
            }
            if ui.button("Not now").clicked() {
                app.health.dismissed_naming = true;
            }
        });
        if let Some((m, false, _)) = &app.form.msg {
            ui.label(RichText::new(m).size(11.0).color(p.crit));
        }
    });
}

/// Runs the rename or save queued by inline_name / naming_card; call where `&mut Shared` is held.
pub fn flush(app: &mut App, s: &mut Shared) {
    if let Some((mac, name)) = app.form.pending_rename.take() {
        if !save_alias(app, s, mac, &name) {
            app.health.editing = Some((mac, name));
        }
    }
    if std::mem::take(&mut app.form.pending_save) && save(app, s) {
        app.health.dismissed_naming = true;
    }
}

/// Writes `mac`'s name (the alias, or the spare name for a spare; empty clears it) through the config save path.
pub fn save_alias(app: &mut App, s: &mut Shared, mac: Mac, text: &str) -> bool {
    let name = text.trim();
    let mut cfg = s.file_cfg.clone();
    if let Some(spare) = cfg.spares.get_mut(&mac) {
        if name.is_empty() {
            cfg.spares.remove(&mac);
        } else {
            *spare = name.to_string();
        }
    } else {
        cfg.devices.entry(mac).or_default().alias = (!name.is_empty()).then(|| name.to_string());
        cfg.devices.retain(|m, d| d.alias.is_some() || d.vendor.is_some() || s.file_cfg.devices.contains_key(m));
    }
    commit(app, s, cfg)
}

pub fn label(ui: &mut Ui, text: &str) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add(egui::Label::new(RichText::new(text).strong()).extend());
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(15.0).strong());
}

fn save(app: &mut App, s: &mut Shared) -> bool {
    let cfg = app.form.v.apply(&s.file_cfg);
    commit(app, s, cfg)
}

/// Validates, saves and adopts `cfg` (keeping the -i / --daemon overrides), resizes the ring if needed and requests a poll.
fn commit(app: &mut App, s: &mut Shared, cfg: Config) -> bool {
    let result = validate(&cfg).and_then(|()| cfg.save(&app.config_path));
    let ok = result.is_ok();
    app.form.msg = Some(match result {
        Ok(()) => {
            if cfg.poll_seconds != s.cfg.poll_seconds || cfg.history_hours != s.cfg.history_hours {
                s.ring.resize(&cfg);
            }
            let mut live = cfg.clone();
            if s.cfg.interfaces != s.file_cfg.interfaces {
                live.interfaces = s.cfg.interfaces.clone();
            }
            if s.cfg.daemon_url != s.file_cfg.daemon_url {
                live.daemon_url = s.cfg.daemon_url.clone();
            }
            s.file_cfg = cfg;
            s.cfg = live;
            app.form.v = Values::new(&s.file_cfg);
            app.refresh.send(()).ok();
            (format!("Saved {}", text::stamp_seconds(SystemTime::now())), true, Instant::now())
        }
        Err(e) => (format!("Save failed: {e:#}"), false, Instant::now()),
    });
    ok
}

fn validate(cfg: &Config) -> Result<()> {
    cfg.check()
}

/// Every /sys/class/net entry of type 1 plus the configured names, each with its checkbox label.
fn ports(st: Option<&NetState>, configured: &[String]) -> Vec<(String, String)> {
    let mut names: Vec<String> = fs::read_dir("/sys/class/net")
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    (fs::read_to_string(e.path().join("type")).ok()?.trim() == "1").then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    names.extend(configured.iter().cloned());
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|n| {
            let text = if st.is_some_and(|st| st.devices.iter().any(|d| d.iface == n)) {
                format!("{n} (wired, adapter answers here)")
            } else if Path::new(&format!("/sys/class/net/{n}/wireless")).exists() {
                format!("{n} (WiFi)")
            } else {
                format!("{n} (wired)")
            };
            (n, text)
        })
        .collect()
}
