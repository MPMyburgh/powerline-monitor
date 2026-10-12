mod adapter;
mod alerts;
mod events;
mod health;
mod link;
mod matrix;
mod overview;
mod poller;
mod settings;
mod shortcuts;
mod theme;
mod tree;
mod visual;
mod widgets;

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Result};
use eframe::egui::{self, Align, Layout, Margin, RichText, Sense, ThemePreference, Ui};
use powerline_core::config::Config;
use powerline_core::model::Mac;
use powerline_core::text;
use serde::{Deserialize, Serialize};

use poller::Shared;
use settings::Form;

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    /// Tab to open
    #[arg(long, value_name = "TAB")]
    pub tab: Option<Tab>,
    /// Preselect a directed link as from_mac,to_mac (default: the slowest link right now)
    #[arg(long, value_name = "FROM,TO", value_parser = parse_link)]
    pub select: Option<(Mac, Mac)>,
    /// Save a PNG of the window after the first poll, then exit
    #[arg(long, value_name = "PATH")]
    pub screenshot: Option<PathBuf>,
    /// Window size (default 1100x700, or the size the window had last time)
    #[arg(long, value_name = "WxH", value_parser = parse_size)]
    pub size: Option<(u32, u32)>,
    /// Open the Adapter tab on this adapter: a name, a MAC, or a MAC suffix of four or more hex digits
    #[arg(long, value_name = "NAME|MAC")]
    pub adapter: Option<String>,
}

fn parse_link(s: &str) -> Result<(Mac, Mac), String> {
    let (a, b) = s.split_once(',').ok_or("expected from_mac,to_mac")?;
    Ok((a.trim().parse().map_err(|e| format!("{e}"))?, b.trim().parse().map_err(|e| format!("{e}"))?))
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WxH")?;
    Ok((w.parse().map_err(|e| format!("{e}"))?, h.parse().map_err(|e| format!("{e}"))?))
}

#[derive(Clone, Copy, PartialEq, Debug, clap::ValueEnum, Serialize, Deserialize)]
pub enum Tab {
    Overview,
    #[value(alias = "matrix")]
    Speeds,
    #[value(alias = "link")]
    History,
    Events,
    Adapter,
    Settings,
}

const TABS: [(&str, Tab); 6] = [
    ("Overview", Tab::Overview),
    ("Speeds", Tab::Speeds),
    ("History", Tab::History),
    ("Events", Tab::Events),
    ("Settings", Tab::Settings),
    ("Adapter", Tab::Adapter),
];
const TAB_HINTS: [&str; 6] = ["1", "2", "3", "4", "5", "6"];

#[derive(Serialize, Deserialize)]
struct Persisted {
    tab: Tab,
    window_secs: u64,
    selected: Option<(Mac, Mac)>,
    theme: Option<ThemePreference>,
    show_details: bool,
    both_dirs: bool,
    dismissed_naming: bool,
}

struct App {
    pub(crate) shared: Arc<Mutex<Shared>>,
    pub(crate) refresh: Sender<()>,
    pub(crate) config_path: PathBuf,
    pub(crate) tab: Tab,
    pub(crate) selected: Option<(Mac, Mac)>,
    pub(crate) window_secs: u64,
    pub(crate) form: Form,
    pub(crate) shot: Option<PathBuf>,
    pub(crate) started: Instant,
    pub(crate) shot_at: Option<Instant>,
    pub(crate) shot_sent: bool,
    pub(crate) health: health::HealthState,
    pub(crate) visual: visual::VisualState,
    pub(crate) ctx: egui::Context,
    pub(crate) size: Option<(u32, u32)>,
}

impl App {
    fn new(cfg: Config, file_cfg: Config, config_path: PathBuf, args: Args, cc: &eframe::CreationContext<'_>) -> App {
        let ctx = cc.egui_ctx.clone();
        let saved: Option<Persisted> = cc.storage.and_then(|s| eframe::get_value(s, "ui"));
        if let Some(t) = saved.as_ref().and_then(|s| s.theme) {
            ctx.set_theme(t);
        }
        let form = Form::new(&file_cfg);
        let mut state = Shared::new(cfg, file_cfg);
        state.adapter_arg = args.adapter.clone();
        let shared = Arc::new(Mutex::new(state));
        let (refresh, rx) = mpsc::channel();
        let worker = shared.clone();
        let poll_ctx = ctx.clone();
        thread::spawn(move || poller::poller(worker, rx, poll_ctx));
        let mut health = health::HealthState::default();
        health.show_details = saved.as_ref().is_some_and(|s| s.show_details);
        health.dismissed_naming = saved.as_ref().is_some_and(|s| s.dismissed_naming);
        App {
            shared,
            refresh,
            config_path,
            tab: args.tab.or(saved.as_ref().map(|s| s.tab)).unwrap_or(Tab::Overview),
            selected: args.select.or(saved.as_ref().and_then(|s| s.selected)),
            window_secs: saved.as_ref().map_or(3600, |s| s.window_secs),
            form,
            shot: args.screenshot,
            started: Instant::now(),
            shot_at: None,
            shot_sent: false,
            health,
            visual: visual::VisualState { both_dirs: saved.as_ref().is_some_and(|s| s.both_dirs), auto: None },
            ctx,
            size: args.size,
        }
    }

    /// Requests a capture 1.5 s after the first successful poll, saves it as PNG and closes the window.
    fn screenshot(&mut self, ctx: &egui::Context, polled: bool) {
        let Some(path) = &self.shot else { return };
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            if let Err(e) = image::save_buffer(path, img.as_raw(), img.width() as u32, img.height() as u32, image::ExtendedColorType::Rgba8) {
                eprintln!("screenshot: {e}");
                std::process::exit(1);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.shot_sent {
            ctx.request_repaint();
        } else if polled {
            let at = *self.shot_at.get_or_insert_with(|| Instant::now() + Duration::from_millis(1500));
            if Instant::now() >= at {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.shot_sent = true;
            }
            ctx.request_repaint();
        } else if self.started.elapsed() >= Duration::from_secs(20) {
            eprintln!("screenshot: no successful poll within 20 s");
            std::process::exit(1);
        }
    }

    fn header(&mut self, ui: &mut Ui, s: &Shared) {
        let p = theme::palette(ui.ctx());
        let frame = egui::Frame::side_top_panel(ui.style()).fill(p.sidebar).inner_margin(Margin::symmetric(12, 4));
        egui::Panel::top("header").exact_size(40.0).frame(frame).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                ui.painter().rect_filled(rect, 5.0, p.accent);
                ui.label(RichText::new("powerline-monitor").size(16.0).strong());
                ui.add_space(12.0);
                widgets::segmented_hinted(ui, &mut self.tab, &TABS, &TAB_HINTS);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let paused = self.health.paused.is_some();
                    if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
                        self.health.paused = (!paused).then(SystemTime::now);
                        if paused {
                            self.refresh.send(()).ok();
                        }
                    }
                    let tip = format!("Ask the adapters now; otherwise every {} s", s.cfg.poll_seconds);
                    if ui.button("Refresh").on_hover_text(tip).clicked() {
                        self.refresh.send(()).ok();
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, s: &Shared) {
        let p = theme::palette(ui.ctx());
        let fill = if ui.visuals().dark_mode { theme::STATUS_BAR } else { p.tile };
        let frame = egui::Frame::new().fill(fill).inner_margin(Margin::symmetric(12, 4));
        egui::Panel::bottom("status").exact_size(24.0).frame(frame).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let mut ifaces: Vec<String> =
                    s.state.as_ref().map(|st| st.devices.iter().map(|d| d.iface.clone()).collect()).unwrap_or_default();
                if ifaces.is_empty() {
                    ifaces = s.cfg.effective_interfaces();
                }
                ifaces.sort();
                ifaces.dedup();
                ui.label(RichText::new(ifaces.join(", ")).monospace().size(12.0));
                let up = s.state.as_ref().map_or(0, |st| st.devices.iter().filter(|d| !d.seen_by_peers_only).count());
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, if s.error.is_some() { p.crit } else { p.ok });
                ui.label(format!("{} up", text::plural(up, "adapter", "adapters")));
                let poll = s.cfg.poll_seconds;
                match s.last_poll {
                    Some(t) if s.attached.is_some() => {
                        let url = s.attached.as_deref().unwrap();
                        let (txt, colour) = match &s.error {
                            Some(e) => (format!("{url} · {e}"), p.crit),
                            None => (format!("{url} · daemon polled {}", text::ago(s.daemon_polled.unwrap_or(t))), p.muted),
                        };
                        ui.label(RichText::new(txt).monospace().color(colour));
                    }
                    Some(_) if self.health.paused.is_some() => {
                        let since = text::stamp_seconds(self.health.paused.unwrap());
                        ui.label(RichText::new(format!("paused since {since}")).monospace().color(p.warn));
                    }
                    Some(t) => {
                        let el = t.elapsed().unwrap_or_default().as_secs();
                        let colour = if el > 2 * poll { p.warn } else { p.muted };
                        let txt = format!("polled {el:>3} s ago · next in {:>3} s", poll.saturating_sub(el));
                        ui.label(RichText::new(txt).monospace().color(colour));
                        let (bar, _) = ui.allocate_exact_size(egui::vec2(120.0, 3.0), Sense::hover());
                        ui.painter().rect_filled(bar, 1.5, p.border);
                        let done = (el as f32 / poll as f32).clamp(0.0, 1.0);
                        ui.painter().rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(120.0 * done, 3.0)), 1.5, p.accent);
                    }
                    None => {
                        ui.label(RichText::new("waiting for the first poll").monospace().weak());
                    }
                }
                if s.polling {
                    ui.add(egui::Spinner::new().size(12.0));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(concat!("powerline-monitor ", env!("CARGO_PKG_VERSION"))).weak());
                });
            });
        });
    }

    fn left_panel(&mut self, ui: &mut Ui, s: &Shared) {
        settings::naming_card(ui, self, s);
        let rect = ui.available_rect_before_wrap();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        egui::ScrollArea::vertical().show(&mut child, |ui| tree::tree(ui, self, s));
        ui.allocate_rect(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), child.min_rect().height())), Sense::hover());
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        theme::apply(ui.ctx());
        if let Some((w, h)) = self.size.take() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w as f32, h as f32)));
        }
        let shared = self.shared.clone();
        let mut s = shared.lock().unwrap();
        ui.ctx().request_repaint_after(if s.polling { Duration::from_millis(250) } else { Duration::from_secs(1) });
        shortcuts::handle(ui.ctx(), self, &s);
        s.paused = self.health.paused.is_some();
        settings::flush(self, &mut s);
        if self.selected.is_none() {
            if let Some(st) = &s.state {
                let macs = st.macs();
                self.selected = macs
                    .iter()
                    .flat_map(|a| macs.iter().map(move |b| (*a, *b)))
                    .filter_map(|(a, b)| st.rate(a, b).map(|r| (r.0, a, b)))
                    .min_by_key(|x| x.0)
                    .map(|(_, a, b)| (a, b));
                self.visual.auto = self.selected;
            }
        }
        self.header(ui, &s);
        self.status_bar(ui, &s);
        let sidebar = egui::Frame::side_top_panel(ui.style()).fill(theme::palette(ui.ctx()).sidebar);
        egui::Panel::left("tree").default_size(300.0).size_range(240.0..=420.0).frame(sidebar).show(ui, |ui| self.left_panel(ui, &s));
        let central = egui::Frame::central_panel(ui.style()).inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 12 });
        egui::CentralPanel::default().frame(central).show(ui, |ui| {
            alerts::show_top(ui, self, &s);
            match self.tab {
                Tab::Overview => overview::show(ui, self, &mut s),
                Tab::Speeds => matrix::show(ui, self, &s),
                Tab::History => link::show(ui, self, &s),
                Tab::Events => events::show(ui, self, &s),
                Tab::Adapter => adapter::show(ui, self, &mut s),
                Tab::Settings => settings::show(ui, self, &mut s),
            }
        });
        self.screenshot(ui.ctx(), s.last_poll.is_some() || s.error.is_some());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.shot.is_some() {
            return;
        }
        let state = Persisted {
            tab: self.tab,
            window_secs: self.window_secs,
            selected: self.selected,
            theme: Some(self.ctx.options(|o| o.theme_preference)),
            show_details: self.health.show_details,
            both_dirs: self.visual.both_dirs,
            dismissed_naming: self.health.dismissed_naming,
        };
        eframe::set_value(storage, "ui", &state);
    }

    fn persist_egui_memory(&self) -> bool {
        self.shot.is_none()
    }
}

fn icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../../../packaging/icons/powerline-monitor-64.png")).expect("embedded icon").into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData { rgba: img.into_raw(), width, height }
}

pub fn run(cfg: Config, file_cfg: Config, config_path: PathBuf, args: Args) -> Result<()> {
    let (w, h) = args.size.unwrap_or((1100, 700));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("powerline-monitor")
            .with_app_id("powerline-monitor")
            .with_icon(icon())
            .with_inner_size([w as f32, h as f32])
            .with_min_inner_size([760.0, 480.0]),
        centered: true,
        persist_window: args.size.is_none(),
        ..Default::default()
    };
    eframe::run_native(
        "powerline-monitor",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(egui::ThemePreference::System);
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(cfg, file_cfg, config_path, args, cc)))
        }),
    )
    .map_err(|e| anyhow!("ui: {e}"))
}
