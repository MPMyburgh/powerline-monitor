use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui;
use powerline_core::client::Client;
use powerline_core::config::Config;
use powerline_core::events::{Event, Tracker};
use powerline_core::history::Ring;
use powerline_core::model::NetState;
use powerline_core::poll::poll_once;

const MAX_EVENTS: usize = 1000;

pub struct Shared {
    pub(crate) cfg: Config,
    pub(crate) file_cfg: Config,
    pub(crate) state: Option<NetState>,
    pub(crate) ring: Ring,
    pub(crate) tracker: Tracker,
    pub(crate) events: Vec<Event>,
    pub(crate) last_poll: Option<SystemTime>,
    pub(crate) error: Option<String>,
    pub(crate) polling: bool,
    pub(crate) paused: bool,
    pub(crate) seeded_from: Option<String>,
    pub(crate) attached: Option<String>,
    pub(crate) daemon_polled: Option<SystemTime>,
    pub(crate) adapter_arg: Option<String>,
}

impl Shared {
    /// `cfg` has the -i / --daemon flags applied; `file_cfg` is the config as written, which saves start from.
    pub fn new(cfg: Config, file_cfg: Config) -> Shared {
        Shared {
            ring: Ring::from_config(&cfg),
            tracker: Tracker::default(),
            cfg,
            file_cfg,
            state: None,
            events: Vec::new(),
            last_poll: None,
            error: None,
            polling: false,
            paused: false,
            seeded_from: None,
            attached: None,
            daemon_polled: None,
            adapter_arg: None,
        }
    }
}

/// Seeds history and events from a running daemon, then polls every cfg.poll_seconds (or reads the daemon's
/// state when daemon_url is set) until the sender is dropped. A refresh polls at once and unpauses.
pub fn poller(shared: Arc<Mutex<Shared>>, rx: Receiver<()>, ctx: egui::Context) {
    let (url, attached) = {
        let s = shared.lock().unwrap();
        (s.cfg.daemon_url.clone().unwrap_or_else(|| format!("http://{}", s.cfg.listen)), s.cfg.daemon_url.is_some())
    };
    let client = Client::new(&url).with_timeout(Duration::from_secs(1));
    let started = SystemTime::now();
    seed(&shared, &client);
    if attached {
        shared.lock().unwrap().attached = Some(format!("attached to {}", client.url()));
    }
    let mut forced = false;
    loop {
        let (cfg, run) = {
            let mut s = shared.lock().unwrap();
            let run = !s.paused || forced;
            s.polling = run;
            (s.cfg.clone(), run)
        };
        if run {
            ctx.request_repaint();
            if attached {
                fetch(&shared, &client, started);
            } else {
                local_poll(&shared, &cfg);
            }
            ctx.request_repaint();
        }
        let wait = if run { cfg.poll_seconds } else { 1 };
        forced = match rx.recv_timeout(Duration::from_secs(wait)) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
    }
}

/// Pulls the daemon's ring and events when one answers; silent otherwise.
fn seed(shared: &Mutex<Shared>, client: &Client) {
    let (Ok(ring), Ok(events)) = (client.history(), client.events(None)) else { return };
    let mut s = shared.lock().unwrap();
    s.ring.merge(&ring);
    s.events = events;
    s.seeded_from = Some(client.url());
}

fn local_poll(shared: &Mutex<Shared>, cfg: &Config) {
    let result = poll_once(cfg);
    let mut guard = shared.lock().unwrap();
    let s = &mut *guard;
    s.polling = false;
    let input: Result<&NetState, String> = result.as_ref().map_err(|e| format!("{e:#}"));
    let ev = s.tracker.observe(input.as_ref().map(|n| *n).map_err(String::as_str), &s.ring, &s.cfg);
    s.events.extend(ev);
    match result {
        Ok(next) => {
            s.ring.push(&next);
            s.last_poll = Some(next.at);
            s.error = next.devices.is_empty().then(|| format!("no adapters found on {}", cfg.effective_interfaces().join(", ")));
            s.state = Some(next);
        }
        Err(e) => {
            let error = format!("{e:#}");
            eprintln!("poll failed: {error}");
            s.error = Some(error);
        }
    }
    trim(s);
}

/// Attached mode: the daemon's state and the events newer than the newest one held here.
fn fetch(shared: &Mutex<Shared>, client: &Client, started: SystemTime) {
    let since = shared.lock().unwrap().events.last().map_or(started, Event::at);
    let state = client.state();
    let polled = client.status().ok().and_then(|st| st.last_poll_at);
    let events = client.events(Some(since));
    let mut guard = shared.lock().unwrap();
    let s = &mut *guard;
    s.polling = false;
    let url = client.url();
    match state {
        Ok(Some(net)) => {
            if s.last_poll != Some(net.at) {
                s.ring.push(&net);
            }
            s.last_poll = Some(net.at);
            s.error = None;
            s.daemon_polled = Some(polled.unwrap_or(net.at));
            s.state = Some(net);
        }
        Ok(None) => s.error = Some(format!("daemon at {url} has not polled yet")),
        Err(e) => s.error = Some(format!("{e:#}")),
    }
    if let Ok(evs) = events {
        for e in evs {
            let dup = s
                .events
                .iter()
                .rev()
                .take_while(|x| x.at() >= e.at())
                .any(|x| x.kind() == e.kind() && x.to_string() == e.to_string());
            if !dup {
                s.events.push(e);
            }
        }
        trim(s);
    }
}

fn trim(s: &mut Shared) {
    let n = s.events.len();
    if n > MAX_EVENTS {
        s.events.drain(..n - MAX_EVENTS);
    }
}
