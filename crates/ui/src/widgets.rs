use eframe::egui::{self, Color32, Response, RichText, Sense, Stroke, TextStyle, TextWrapMode, Ui, WidgetText};
use powerline_core::model::{Mac, Role};

use crate::theme::{self, Palette};

pub fn badge(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) -> Response {
    let galley = WidgetText::from(RichText::new(text).size(11.0).strong().color(fg))
        .into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Small);
    let size = egui::vec2(galley.size().x.max(32.0) + 12.0, galley.size().y + 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 3.0, fill);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    }
    response
}

pub fn mac(m: Mac) -> RichText {
    RichText::new(m.to_string()).monospace().size(13.0)
}

pub fn mac_short(m: Mac) -> RichText {
    RichText::new(format!("..{:02x}:{:02x}", m.0[4], m.0[5])).monospace().size(11.0).weak()
}

/// Draws one button per option; returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(&str, T)]) -> bool {
    segmented_hinted(ui, value, options, &[])
}

/// `segmented` with an on-hover hint per option, e.g. its shortcut key.
pub fn segmented_hinted<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(&str, T)], hints: &[&str]) -> bool {
    let p = theme::palette(ui.ctx());
    let selected_text = ui.visuals().selection.stroke.color;
    let mut changed = false;
    egui::Frame::new()
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(6)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, (label, opt)) in options.iter().enumerate() {
                    let selected = *value == *opt;
                    let mut text = RichText::new(*label);
                    if selected {
                        text = text.color(selected_text);
                    }
                    let mut b = egui::Button::new(text)
                        .min_size(egui::vec2(0.0, 24.0))
                        .stroke(Stroke::NONE)
                        .frame_when_inactive(selected);
                    if selected {
                        b = b.fill(p.selection);
                    }
                    let mut r = ui.add(b);
                    if let Some(h) = hints.get(i) {
                        r = r.on_hover_text(*h);
                    }
                    if r.clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}

pub fn stat_tile(ui: &mut Ui, label: &str, value: &str, tooltip: Option<&str>) {
    let p = theme::palette(ui.ctx());
    let r = egui::Frame::new()
        .fill(p.tile)
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_min_width(100.0);
                ui.label(RichText::new(value).size(20.0).strong());
                ui.label(RichText::new(label).size(11.0).color(p.muted));
            });
        })
        .response;
    if let Some(t) = tooltip {
        r.on_hover_text(t);
    }
}

pub fn role_badge(ui: &mut Ui, role: Role, palette: &Palette) {
    let (fill, fg) = theme::role_colours(role, palette);
    let r = badge(ui, role.human(), fill, fg);
    if let Some(hint) = role_hint(role) {
        r.on_hover_text(hint);
    }
}

pub fn role_hint(role: Role) -> Option<&'static str> {
    match role {
        Role::Cco | Role::BackupCco => Some("Coordinator: the adapter that runs the powerline network; Backup takes over if it drops"),
        Role::Sta => Some("Member: an ordinary adapter"),
        Role::UnassocCco | Role::UnassocSta | Role::Unknown => None,
    }
}
