use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{RichText, Ui};
use egui_plot::{Corner, GridInput, GridMark, HLine, HoverPosition, Legend, Line, LineStyle, Plot, PlotPoints};
use powerline_core::text;

use crate::poller::Shared;
use crate::{health, theme, widgets, App};

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

pub fn show(ui: &mut Ui, app: &mut App, s: &Shared) {
    let Some((from, to)) = app.selected else {
        ui.label("Select a link in the Speeds tab.");
        return;
    };
    let cfg = &s.cfg;
    let p = theme::palette(ui.ctx());
    let prefix = if app.visual.auto == Some((from, to)) { "Slowest link right now · " } else { "" };
    ui.label(RichText::new(format!("{prefix}{} → {}", cfg.name(from), cfg.name(to))).size(20.0).strong());
    ui.label(RichText::new(format!("{from} → {to}")).monospace().size(12.0).weak());
    if let (Some(url), Some(oldest)) = (&s.seeded_from, s.ring.oldest()) {
        ui.label(RichText::new(format!("History from the powerline-monitor service at {url}, back to {}", text::stamp(oldest))).weak());
    }
    ui.add_space(4.0);
    let mut windows = vec![("15 min", 900u64), ("1 h", 3600), ("6 h", 21600), ("24 h", 86400)];
    if s.ring.oldest().is_some_and(|o| o.elapsed().unwrap_or_default() > Duration::from_secs(86400)) {
        windows.push(("7 d", 604800));
    }
    let csv = ui
        .horizontal(|ui| {
            widgets::segmented(ui, &mut app.window_secs, &windows);
            ui.button("Copy CSV").clicked()
        })
        .inner;
    if csv {
        health::copy(ui.ctx(), app, s.ring.to_csv(cfg, Some((from, to))));
    }
    ui.add_space(8.0);
    if s.ring.len() <= 1 {
        ui.label(format!(
            "Collecting history: {} so far, one every {} s. Leave the window open, or enable the powerline-monitor service to keep history between launches.",
            text::plural(s.ring.len(), "reading", "readings"),
            cfg.poll_seconds
        ));
        return;
    }
    let series = s.ring.series(from, to);
    let Some(stats) = s.ring.stats(from, to) else {
        ui.label(format!("No readings for this link yet: {} has not reported a rate to {}.", cfg.name(from), cfg.name(to)));
        return;
    };
    let span = series.last().and_then(|l| l.0.duration_since(series[0].0).ok()).unwrap_or_default().as_secs();
    let window_label = windows.iter().find(|(_, w)| *w == app.window_secs).map_or_else(|| text::duration(app.window_secs), |(l, _)| l.to_string());
    ui.horizontal(|ui| {
        widgets::stat_tile(ui, "Now", &stats.now.to_string(), Some("Latest reading, Mbit/s"));
        widgets::stat_tile(ui, "Lowest", &stats.min.to_string(), Some("Lowest reading in the history this window keeps"));
        widgets::stat_tile(ui, "Usual", &stats.median.to_string(), Some("Median of all kept readings"));
        let readings = if stats.samples == 1 { "reading" } else { "readings" };
        widgets::stat_tile(ui, &format!("{readings} over {}", text::duration(span)), &stats.samples.to_string(), None);
        widgets::stat_tile(ui, "Window", &window_label, None);
    });
    ui.add_space(8.0);
    let now = epoch_secs(SystemTime::now());
    let start = now - app.window_secs as f64;
    let pick = |f: fn(&(SystemTime, u16, u16)) -> u16| -> Vec<[f64; 2]> {
        series.iter().map(|x| [epoch_secs(x.0), f64::from(f(x))]).filter(|pt| pt[0] >= start).collect()
    };
    let tx = pick(|x| x.1);
    let rx = pick(|x| x.2);
    let max_sample = tx.iter().chain(&rx).map(|pt| pt[1]).fold(0.0, f64::max);
    let grid = if ui.visuals().dark_mode { theme::GRID_DARK } else { theme::GRID_LIGHT };
    let window = app.window_secs;
    let sends = format!("{} sends", cfg.name(from));
    let receives = format!("{} receives", cfg.name(from));
    Plot::new("link")
        .legend(Legend::default().position(Corner::LeftBottom).background_alpha(0.6))
        .label_formatter(|pos| match pos {
            HoverPosition::NearDataPoint { plot_name, position, .. } => Some(format!(
                "{plot_name}\n{}  {:.0} Mbit/s",
                text::stamp(UNIX_EPOCH + Duration::from_secs_f64(position.x.max(0.0))),
                position.y
            )),
            HoverPosition::Elsewhere { .. } => None,
        })
        .x_grid_spacer(move |input| grid_marks(input, window))
        .x_axis_formatter(|mark, _| axis_label(mark.value))
        .include_x(start)
        .include_x(now)
        .include_y(0.0)
        .include_y(600.0)
        .include_y(max_sample * 1.1)
        .y_axis_min_width(44.0)
        .y_axis_label("Mbit/s")
        .show_background(false)
        .grid_color(grid)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show(ui, |plot| {
            plot.line(Line::new(sends, PlotPoints::from(tx)).color(p.accent).width(2.0).fill(0.0).fill_alpha(0.08));
            plot.line(Line::new(receives, PlotPoints::from(rx)).color(p.ok).width(2.0));
            plot.hline(HLine::new("usual", f64::from(stats.median)).color(p.muted).width(1.0).style(LineStyle::dashed_loose()));
            plot.hline(HLine::new("poor below", f64::from(cfg.alerts.floor_mbps)).color(theme::POOR).width(1.0));
        });
}

pub fn epoch_secs(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Marks on local-time boundaries; (minor, major) step in seconds picked by the window length.
fn grid_marks(input: GridInput, window: u64) -> Vec<GridMark> {
    let (minor, major) = match window {
        ..=900 => (60.0, 300.0),
        901..=3600 => (300.0, 900.0),
        3601..=21600 => (1800.0, 3600.0),
        21601..=86400 => (3600.0, 21600.0),
        _ => (21600.0, 86400.0),
    };
    let (lo, hi) = input.bounds;
    let off = text::local_tm(lo as i64).tm_gmtoff as f64;
    let mut v = ((lo + off) / minor).floor() * minor - off;
    let mut marks = Vec::new();
    while v <= hi {
        if v >= lo {
            let step_size = if ((v + off) % major).abs() < 1.0 { major } else { minor };
            marks.push(GridMark { value: v, step_size });
        }
        v += minor;
    }
    marks
}

fn axis_label(secs: f64) -> String {
    let tm = text::local_tm(secs as i64);
    let hm = text::hm(secs);
    if tm.tm_hour == 0 && tm.tm_min == 0 {
        format!("{hm} {} {:02}", DAYS[tm.tm_wday as usize % 7], tm.tm_mday)
    } else {
        hm
    }
}
