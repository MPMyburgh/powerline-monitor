use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use powerline_core::config::Config;
use powerline_core::events::{Event, Tracker};
use powerline_core::history::{history_path, tracker_path, Ring};
use powerline_core::keys;
use powerline_core::model::{rfc3339, Mac, NetState, Nid, Role};
use powerline_core::poll::Poller;
use powerline_core::text;
use serde::Serialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const HOOK_DEADLINE: Duration = Duration::from_secs(10);

/// stderr line that is dropped, not a panic, when stderr is gone (journald restart, closed pipe).
macro_rules! log {
    ($($arg:tt)*) => {{ let _ = writeln!(std::io::stderr(), $($arg)*); }};
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    /// Address to serve on (default: listen from the config)
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PollMeta {
    pub last_poll_ok: bool,
    pub polls: u64,
    pub last_poll_at: Option<SystemTime>,
    pub poll_duration: f64,
}

struct State {
    net: Option<NetState>,
    ring: Ring,
    tracker: Tracker,
    events: VecDeque<Event>,
    event_counts: BTreeMap<String, u64>,
    meta: PollMeta,
    state_dir: PathBuf,
    host: String,
    hook: Option<Sender<Event>>,
}

impl State {
    /// Appends the payload to events.jsonl and stderr, counts it, keeps the last 500 and forwards it to the hook.
    fn record(&mut self, ev: Event, cfg: &Config) {
        let line = ev.payload(cfg, &self.host).to_string();
        log!("{line}");
        let path = self.state_dir.join("events.jsonl");
        if let Err(e) = OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "{line}")) {
            log!("warning: writing {}: {e}", path.display());
        }
        *self.event_counts.entry(ev.kind().to_string()).or_default() += 1;
        if let Some(tx) = self.hook.as_ref().filter(|_| ev.severity() >= cfg.hooks.min_severity) {
            let _ = tx.send(ev.clone());
        }
        self.events.push_back(ev);
        while self.events.len() > 500 {
            self.events.pop_front();
        }
    }
}

fn warn(r: Result<()>) {
    if let Err(e) = r {
        log!("warning: {e:#}");
    }
}

fn load_ring(cfg: &Config) -> Ring {
    let path = history_path();
    let mut ring = match path.exists().then(|| Ring::load(&path)) {
        Some(Ok(r)) => r,
        Some(Err(e)) => {
            log!("warning: {e:#}; starting a new history");
            Ring::from_config(cfg)
        }
        None => Ring::from_config(cfg),
    };
    ring.resize(cfg);
    ring
}

fn load_tracker() -> Tracker {
    let path = tracker_path();
    if !path.exists() {
        return Tracker::default();
    }
    fs::read(&path).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice(&b)?)).unwrap_or_else(|e| {
        log!("warning: reading {}: {e}; starting a new tracker", path.display());
        Tracker::default()
    })
}

/// The last 500 events of state_dir/events.jsonl, so /api/events survives a restart.
fn load_events(dir: &Path) -> VecDeque<Event> {
    let text = fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default();
    let mut v: VecDeque<Event> = text.lines().filter_map(Event::from_payload).collect();
    let extra = v.len().saturating_sub(500);
    v.drain(..extra);
    v
}

fn save_tracker(t: &Tracker) -> Result<()> {
    let path = tracker_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(t)?).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
}

fn hostname() -> String {
    std::env::var("POWERLINE_MONITOR_HOST")
        .ok()
        .or_else(|| fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// The serde string of a unit enum, e.g. Severity::Warning -> "warning".
fn tag(v: impl Serialize) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

pub fn run(cfg: &Config, config_path: &Path, args: &Args) -> Result<()> {
    let listen = args.listen.clone().unwrap_or_else(|| cfg.listen.clone());
    let state_dir = Config::ensure_state_dir()?;
    let server = (!listen.is_empty()).then(|| Server::http(&listen).map_err(|e| anyhow!("listen on {listen}: {e}"))).transpose()?;
    let host = hostname();
    let (hook_tx, hook_thread) = (!cfg.hooks.exec.is_empty())
        .then(|| {
            let (tx, rx) = mpsc::channel::<Event>();
            let (cfg, host) = (cfg.clone(), host.clone());
            (tx, thread::spawn(move || hook_loop(rx, &cfg, &host)))
        })
        .unzip();
    let state = Arc::new(Mutex::new(State {
        net: None,
        ring: load_ring(cfg),
        tracker: load_tracker(),
        events: load_events(&state_dir),
        event_counts: BTreeMap::new(),
        meta: PollMeta::default(),
        state_dir: state_dir.clone(),
        host,
        hook: hook_tx,
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            stop.store(true, Ordering::SeqCst);
            let _ = wake_tx.send(());
        })?;
    }
    let poller = thread::spawn({
        let cfg = cfg.clone();
        let state = state.clone();
        move || poll_loop(&cfg, &state, &wake_rx)
    });
    let hook = if cfg.hooks.exec.is_empty() { "none".to_string() } else { cfg.hooks.exec.join(" ") };
    let metrics = if listen.is_empty() { "metrics off".to_string() } else { format!("metrics http://{listen}/metrics") };
    log!(
        "powerline-monitor daemon {VERSION}\nconfig {}\nstate {}\ninterfaces {}\npoll every {} s\n{metrics}\nhook {hook} (min severity {})",
        config_path.display(),
        state_dir.display(),
        cfg.effective_interfaces().join(", "),
        cfg.poll_seconds,
        tag(cfg.hooks.min_severity)
    );
    while !stop.load(Ordering::SeqCst) {
        match &server {
            Some(server) => {
                if let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(500)) {
                    handle(req, &state, cfg);
                }
            }
            None => thread::sleep(Duration::from_millis(500)),
        }
    }
    poller.join().ok();
    {
        let mut st = state.lock().unwrap();
        warn(st.ring.save(&history_path()));
        warn(save_tracker(&st.tracker));
        st.hook = None;
    }
    if let Some(h) = hook_thread {
        h.join().ok();
    }
    Ok(())
}

fn poll_loop(cfg: &Config, state: &Mutex<State>, wake: &Receiver<()>) {
    let interval = Duration::from_secs(cfg.poll_seconds);
    let mut poller = Poller::new(cfg);
    loop {
        let t0 = Instant::now();
        let res = poller.poll(cfg);
        let elapsed = t0.elapsed();
        let mut guard = state.lock().unwrap();
        let st = &mut *guard;
        st.meta.polls += 1;
        st.meta.last_poll_at = Some(SystemTime::now());
        st.meta.poll_duration = elapsed.as_secs_f64();
        st.meta.last_poll_ok = res.is_ok();
        if st.meta.polls == 1 {
            first_poll_line(&res, cfg, elapsed);
        }
        let input: Result<&NetState, String> = res.as_ref().map_err(|e| format!("{e:#}"));
        let evs = st.tracker.observe(input.as_ref().map(|n| *n).map_err(String::as_str), &st.ring, cfg);
        if let Ok(net) = res {
            st.ring.push(&net);
            st.net = Some(net);
        }
        let fired = !evs.is_empty();
        for ev in evs {
            st.record(ev, cfg);
        }
        if st.meta.polls.is_multiple_of(10) {
            warn(st.ring.save(&history_path()));
        }
        if fired {
            warn(save_tracker(&st.tracker));
        }
        drop(guard);
        if !matches!(wake.recv_timeout(interval), Err(RecvTimeoutError::Timeout)) {
            return;
        }
    }
}

fn first_poll_line(res: &Result<NetState>, cfg: &Config, elapsed: Duration) {
    match res {
        Ok(net) => {
            let answering = net.devices.iter().filter(|d| !d.seen_by_peers_only).count();
            let all: BTreeSet<Mac> = net.devices.iter().map(|d| d.mac).chain(cfg.devices.keys().copied()).collect();
            let via = match net.devices.iter().find(|d| d.local) {
                Some(d) => format!("{} ({} wired)", d.iface, cfg.name(d.mac)),
                None => cfg.effective_interfaces().join(", "),
            };
            log!("poll 1: {answering} of {} adapters via {via} in {} ms", all.len(), elapsed.as_millis());
        }
        Err(e) => log!("poll 1: {e:#}"),
    }
}

/// Runs cfg.hooks.exec per event with the payload on stdin, killing it after HOOK_DEADLINE.
fn hook_loop(rx: Receiver<Event>, cfg: &Config, host: &str) {
    let exec = &cfg.hooks.exec;
    for ev in rx {
        let kind = ev.kind();
        let spawned = unsafe {
            Command::new(&exec[0])
                .args(&exec[1..])
                .arg(kind)
                .env("POWERLINE_MONITOR_EVENT", kind)
                .env("POWERLINE_MONITOR_SEVERITY", tag(ev.severity()))
                .env("POWERLINE_MONITOR_SUMMARY", ev.summary(&|m| cfg.name(m)))
                .stdin(Stdio::piped())
                .pre_exec(|| {
                    libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong);
                    Ok(())
                })
                .spawn()
        };
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                log!("hook_failed kind={kind} status={e}");
                continue;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(ev.payload(cfg, host).to_string().as_bytes());
        }
        let deadline = Instant::now() + HOOK_DEADLINE;
        let failure = loop {
            match child.try_wait() {
                Ok(Some(s)) if s.success() => break None,
                Ok(Some(s)) => break Some(s.code().map_or_else(|| s.to_string(), |c| c.to_string())),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Some("timeout".to_string());
                }
                Err(e) => break Some(e.to_string()),
            }
        };
        if let Some(status) = failure {
            log!("hook_failed kind={kind} status={status}");
        }
    }
}

const TEXT: &str = "text/plain; charset=utf-8";
const JSON: &str = "application/json";

fn handle(req: Request, state: &Mutex<State>, cfg: &Config) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let since = query.split('&').find_map(|kv| kv.strip_prefix("since=")).map(text::parse_since);
    let (status, ctype, body) = {
        let st = state.lock().unwrap();
        if *req.method() != Method::Get {
            (405, TEXT, "method not allowed\n".to_string())
        } else {
            match path {
                "/metrics" => (
                    200,
                    "text/plain; version=0.0.4; charset=utf-8",
                    metrics(st.net.as_ref(), cfg, &st.event_counts, &st.meta),
                ),
                "/api/state" => match &st.net {
                    Some(net) => (200, JSON, serde_json::to_string(net).unwrap()),
                    None => (503, JSON, r#"{"error":"no poll yet"}"#.to_string()),
                },
                "/api/history" => (200, JSON, serde_json::to_string(&st.ring).unwrap()),
                "/api/history.csv" => (200, "text/csv; charset=utf-8", st.ring.to_csv(cfg, None)),
                "/api/events" => match since {
                    Some(Err(e)) => (400, JSON, json!({ "error": e.to_string() }).to_string()),
                    other => {
                        let from = other.and_then(Result::ok).unwrap_or(UNIX_EPOCH);
                        let evs: Vec<&Event> = st.events.iter().filter(|e| e.at() >= from).collect();
                        (200, JSON, serde_json::to_string(&evs).unwrap())
                    }
                },
                "/api/status" => (
                    200,
                    JSON,
                    json!({
                        "version": VERSION,
                        "interfaces": cfg.effective_interfaces(),
                        "poll_seconds": cfg.poll_seconds,
                        "last_poll_at": st.meta.last_poll_at.map(rfc3339::format),
                        "last_poll_ok": st.meta.last_poll_ok,
                        "polls": st.meta.polls,
                        "poll_duration": st.meta.poll_duration,
                        "state_dir": st.state_dir.display().to_string(),
                        "hook": st.hook.is_some(),
                    })
                    .to_string(),
                ),
                "/healthz" => (200, TEXT, "ok\n".to_string()),
                _ => (404, TEXT, "not found\n".to_string()),
            }
        }
    };
    let resp = Response::from_string(body)
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", ctype).unwrap());
    let _ = req.respond(resp);
}

fn head(o: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(o, "# HELP {name} {help}\n# TYPE {name} {kind}");
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn role(r: Role) -> &'static str {
    match r {
        Role::Cco => "CCO",
        Role::BackupCco => "BACKUP_CCO",
        Role::Sta => "STA",
        Role::UnassocCco => "UNASSOC_CCO",
        Role::UnassocSta => "UNASSOC_STA",
        Role::Unknown => "UNKNOWN",
    }
}

fn av_label(v: u8) -> String {
    match v {
        0 => "1.1".to_string(),
        1 => "2.0".to_string(),
        n => n.to_string(),
    }
}

pub fn metrics(net: Option<&NetState>, cfg: &Config, counts: &BTreeMap<String, u64>, meta: &PollMeta) -> String {
    let mut o = String::new();
    head(&mut o, "homeplug_up", "gauge", "1 if the last poll succeeded");
    let _ = writeln!(o, "homeplug_up {}", u8::from(meta.last_poll_ok));
    head(&mut o, "homeplug_last_poll_timestamp_seconds", "gauge", "Unix time of the last poll");
    let ts = meta.last_poll_at.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let _ = writeln!(o, "homeplug_last_poll_timestamp_seconds {ts}");
    head(&mut o, "homeplug_poll_duration_seconds", "gauge", "Duration of the last poll");
    let _ = writeln!(o, "homeplug_poll_duration_seconds {:.3}", meta.poll_duration);
    head(&mut o, "homeplug_polls_total", "counter", "Polls attempted");
    let _ = writeln!(o, "homeplug_polls_total {}", meta.polls);
    head(&mut o, "homeplug_events_total", "counter", "Events by kind");
    for (kind, n) in counts {
        let _ = writeln!(o, "homeplug_events_total{{kind=\"{kind}\"}} {n}");
    }
    let Some(net) = net else { return o };
    head(&mut o, "homeplug_default_key", "gauge", "1 if the network uses the default key HomePlugAV");
    let _ = writeln!(o, "homeplug_default_key {}", u8::from(net.default_key()));
    head(&mut o, "homeplug_network_default_key", "gauge", "1 if the NID derives from the default key HomePlugAV");
    let nids: BTreeSet<Nid> = net.snapshots.iter().map(|s| s.nid).collect();
    for nid in &nids {
        let _ = writeln!(
            o,
            "homeplug_network_default_key{{nid=\"{nid}\"}} {}",
            u8::from(keys::is_default_nid(nid))
        );
    }
    head(&mut o, "homeplug_unknown_stations", "gauge", "Stations on the network that are not in the config");
    let unknown = if cfg.devices.is_empty() { 0 } else { net.devices.iter().filter(|d| !cfg.is_owned(d.mac)).count() };
    let _ = writeln!(o, "homeplug_unknown_stations {unknown}");
    head(&mut o, "homeplug_networks_heard", "gauge", "Other powerline networks heard on this wiring");
    let own = net.nid();
    let heard: BTreeSet<Nid> =
        net.snapshots.iter().flat_map(|s| s.neighbour_nets.iter()).map(|n| n.nid).filter(|n| Some(*n) != own).collect();
    let _ = writeln!(o, "homeplug_networks_heard {}", heard.len());
    head(&mut o, "homeplug_device_info", "gauge", "Adapter identity");
    for d in &net.devices {
        let oui = d.caps.as_ref().map(|c| format!("{:02x}:{:02x}:{:02x}", c.oui[0], c.oui[1], c.oui[2])).unwrap_or_default();
        let av = d.caps.as_ref().map(|c| av_label(c.av_version)).unwrap_or_default();
        let firmware = net.snapshot(d.mac).and_then(|s| s.firmware.as_deref()).unwrap_or("");
        let _ = writeln!(
            o,
            "homeplug_device_info{{device_addr=\"{}\",interface=\"{}\",vendor=\"{}\",hfid=\"{}\",local=\"{}\",alias=\"{}\",firmware=\"{}\",oui=\"{oui}\",av=\"{av}\"}} 1",
            d.mac,
            d.iface,
            d.vendor,
            esc(d.hfid.as_deref().unwrap_or("")),
            d.local,
            esc(&cfg.name(d.mac)),
            esc(firmware)
        );
    }
    head(&mut o, "homeplug_device_up", "gauge", "1 if the adapter answered the last poll");
    let macs: BTreeSet<Mac> = net.devices.iter().map(|d| d.mac).chain(cfg.devices.keys().copied()).collect();
    for mac in macs {
        let _ = writeln!(o, "homeplug_device_up{{device_addr=\"{mac}\"}} {}", u8::from(net.snapshot(mac).is_some()));
    }
    head(&mut o, "homeplug_device_uptime_seconds", "gauge", "Seconds since the adapter booted");
    for s in net.snapshots.iter().filter(|s| s.uptime_secs.is_some()) {
        let _ = writeln!(o, "homeplug_device_uptime_seconds{{device_addr=\"{}\"}} {}", s.device, s.uptime_secs.unwrap());
    }
    head(&mut o, "homeplug_bridged_stations", "gauge", "Ethernet stations bridged behind the adapter");
    for s in &net.snapshots {
        let _ = writeln!(o, "homeplug_bridged_stations{{device_addr=\"{}\"}} {}", s.device, s.bridged.len());
    }
    head(&mut o, "homeplug_network_info", "gauge", "Network membership");
    for s in &net.snapshots {
        let cco_addr = s.cco.map(|m| m.to_string()).unwrap_or_default();
        let cco_tei = s.cco.and_then(|m| net.snapshot(m)).map(|c| c.tei.to_string()).unwrap_or_default();
        let _ = writeln!(
            o,
            "homeplug_network_info{{device_addr=\"{}\",nid=\"{}\",snid=\"{}\",tei=\"{}\",role=\"{}\",cco_addr=\"{}\",cco_tei=\"{}\"}} 1",
            s.device,
            s.nid,
            s.snid,
            s.tei,
            role(s.role),
            cco_addr,
            cco_tei
        );
    }
    head(&mut o, "homeplug_station_signal_level", "gauge", "Signal level band heard from the peer: 1 strongest, 15 weakest, 0 unknown");
    for s in &net.snapshots {
        for h in &s.heard {
            let _ = writeln!(
                o,
                "homeplug_station_signal_level{{device_addr=\"{}\",peer_addr=\"{}\"}} {}",
                s.device, h.mac, h.signal_level
            );
        }
    }
    head(&mut o, "homeplug_station_link_info", "gauge", "Spec, MIMO mode and beamforming of the TX link to the peer");
    for s in &net.snapshots {
        for l in &s.links {
            let _ = writeln!(
                o,
                "homeplug_station_link_info{{device_addr=\"{}\",peer_addr=\"{}\",spec=\"{}\",mimo=\"{}\",beamforming=\"{}\"}} 1",
                s.device,
                l.peer,
                tag(l.tx_flags.spec),
                tag(l.tx_flags.mimo),
                l.tx_flags.beamforming
            );
        }
    }
    for (name, dir) in [("homeplug_station_tx_rate_bytes", "TX"), ("homeplug_station_rx_rate_bytes", "RX")] {
        head(&mut o, name, "gauge", &format!("{dir} PHY rate to the peer in bytes/s"));
        for s in &net.snapshots {
            for l in &s.links {
                let mbps = if dir == "TX" { l.tx_mbps } else { l.rx_mbps };
                let _ = writeln!(
                    o,
                    "{name}{{device_addr=\"{}\",nid=\"{}\",peer_addr=\"{}\"}} {}",
                    s.device,
                    s.nid,
                    l.peer,
                    u64::from(mbps) * 125_000
                );
            }
        }
    }
    o
}
