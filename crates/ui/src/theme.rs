use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference, Ui};
use powerline_core::model::Role;

pub struct Palette {
    pub panel: Color32,
    pub sidebar: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub crit: Color32,
    pub tile: Color32,
    pub selection: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const GOOD: Color32 = hex(0x009E73);
pub const REDUCED: Color32 = hex(0xE69F00);
pub const POOR: Color32 = hex(0xD55E00);
pub const NODATA: Color32 = hex(0x3A3F4B);
pub const NODATA_TEXT: Color32 = hex(0xAEB7C4);
pub const STATUS_BAR: Color32 = hex(0x161920);
pub const GRID_DARK: Color32 = Color32::from_gray(48);
pub const GRID_LIGHT: Color32 = Color32::from_gray(200);
pub const WIRED_FILL: Color32 = hex(0x233044);
pub const WIRED_FG: Color32 = hex(0x9CC4FF);
pub const KEY_FILL_DARK: Color32 = hex(0x3A2A10);
pub const KEY_FILL_LIGHT: Color32 = hex(0xFFF8C5);
pub const ERROR_FILL: Color32 = hex(0x3A1A1A);
pub const INFO_CHIP: (Color32, Color32) = (hex(0x233044), hex(0x9CC4FF));
pub const WARNING_CHIP: (Color32, Color32) = (hex(0x3D2E0A), hex(0xF0C048));
pub const CRITICAL_CHIP: (Color32, Color32) = (hex(0x4A1A1A), hex(0xFF8A80));
pub const CRITICAL_ROW_DARK: Color32 = hex(0x2A1414);
pub const CRITICAL_ROW_LIGHT: Color32 = hex(0xFFEBE9);

pub fn dark() -> Palette {
    Palette {
        panel: hex(0x1A1D23),
        sidebar: hex(0x1E2128),
        border: hex(0x30363D),
        text: hex(0xE6EDF3),
        muted: hex(0x8B949E),
        accent: hex(0x58A6FF),
        ok: hex(0x3FB950),
        warn: hex(0xD29922),
        crit: hex(0xF85149),
        tile: hex(0x262A33),
        selection: hex(0x2F5FA8),
    }
}

pub fn light() -> Palette {
    Palette {
        panel: hex(0xF6F8FA),
        sidebar: hex(0xFFFFFF),
        border: hex(0xD0D7DE),
        text: hex(0x1F2328),
        muted: hex(0x57606A),
        accent: hex(0x0969DA),
        ok: hex(0x1A7F37),
        warn: hex(0x9A6700),
        crit: hex(0xCF222E),
        tile: hex(0xEAEEF2),
        selection: hex(0xDDF4FF),
    }
}

pub fn palette(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        Theme::Dark => dark(),
        Theme::Light => light(),
    }
}

/// (fill, foreground) of the role badge.
pub fn role_colours(role: Role, palette: &Palette) -> (Color32, Color32) {
    match role {
        Role::Cco => (hex(0x1F4E8C), hex(0xCFE3FF)),
        Role::BackupCco => (hex(0x175C63), hex(0xBDEFF2)),
        Role::Sta => (hex(0x3A3F4B), hex(0xD5DAE2)),
        Role::UnassocCco | Role::UnassocSta | Role::Unknown => (hex(0x2A2E36), palette.muted),
    }
}

/// Installs the palette, type scale and spacing on both egui themes.
pub fn apply(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let is_dark = theme == Theme::Dark;
        let p = if is_dark { dark() } else { light() };
        let (extreme, inactive, hovered, active) = if is_dark {
            (hex(0x111318), hex(0x2A2F3A), hex(0x343A47), hex(0x3E4553))
        } else {
            (hex(0xFFFFFF), hex(0xEAEEF2), hex(0xE1E6EB), hex(0xD0D7DE))
        };
        ctx.style_mut_of(theme, |style| {
            let v = &mut style.visuals;
            v.panel_fill = p.panel;
            v.window_fill = p.sidebar;
            v.extreme_bg_color = extreme;
            v.faint_bg_color = p.tile;
            v.code_bg_color = p.tile;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
            for (w, fill) in [(&mut v.widgets.inactive, inactive), (&mut v.widgets.hovered, hovered), (&mut v.widgets.active, active)] {
                w.weak_bg_fill = fill;
                w.bg_fill = fill;
            }
            for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = CornerRadius::same(4);
                w.fg_stroke.color = p.text;
            }
            v.selection.bg_fill = p.selection;
            v.selection.stroke.color = if is_dark { Color32::WHITE } else { p.text };
            v.hyperlink_color = p.accent;
            v.weak_text_color = Some(p.muted);
            style.text_styles = [
                (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
        });
    }
}

/// Dark / Light / System combo box bound to the context's theme preference.
pub fn appearance_row(ui: &mut Ui, ctx: &egui::Context) {
    let current = ctx.options(|o| o.theme_preference);
    let mut pref = current;
    let options = [("Dark", ThemePreference::Dark), ("Light", ThemePreference::Light), ("System", ThemePreference::System)];
    let label = options.iter().find(|(_, v)| *v == current).map_or("System", |(l, _)| *l);
    egui::ComboBox::from_id_salt("appearance").selected_text(label).show_ui(ui, |ui| {
        for (name, v) in options {
            ui.selectable_value(&mut pref, v, name);
        }
    });
    if pref != current {
        ctx.set_theme(pref);
    }
}

/// Adds the bundled monospace face as the last proportional fallback so → ↓ ● render in body text.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
}
