use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use clap::Subcommand;
use powerline_core::client::Client;
use powerline_core::config::Config;
use powerline_core::events::{Event, Severity};
use powerline_core::history::{history_path, Poll, Ring};
use powerline_core::model::Mac;
use powerline_core::poll::{poll_once, Poller};
use powerline_core::render::sparkline;
use powerline_core::text;
use powerline_daemon::PollMeta;
use serde_json::json;

const CELLS: usize = 48;
const HOURS: usize = 24;

#[derive(clap::Args, Debug)]
pub struct HistoryArgs {
    /// Two adapters (name, MAC or MAC suffix) for the hourly grid of that pair; none for every link
    #[arg(value_name = "ADAPTER", num_args = 0..=2)]
    pub adapters: Vec<String>,
    /// Readings since: 30m, 6h, 2d, unix seconds or RFC 3339 (default 24h)
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Print JSON instead of the table
    #[arg(long)]
    pub json: bool,
    /// Print every reading as CSV
    #[arg(long)]
    pub csv: bool,
}

#[derive(clap::Args, Debug)]
pub struct EventsArgs {
    /// Events since: 30m, 6h, 2d, unix seconds or RFC 3339 (default: all the daemon holds)
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Lowest severity to show: info, warning or critical
    #[arg(long, value_name = "LEVEL", value_parser = parse_severity)]
    pub severity: Option<Severity>,
    /// Print the hook payload JSON, one line per event
    #[arg(long)]
    pub json: bool,
    /// Keep printing new events, checking every poll_seconds
    #[arg(long)]
    pub follow: bool,
}

#[derive(clap::Args, Debug)]
pub struct RenameArgs {
    /// The adapter: its current name, its MAC, or the last four or more hex digits of the MAC
    pub adapter: String,
    /// The new name, 1 to 32 characters. A local label in the config only; the adapter itself is not written
    #[arg(required_unless_present = "clear")]
    pub name: Option<String>,
    /// Remove the name instead
    #[arg(long, conflicts_with = "name")]
    pub clear: bool,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Check a config file for syntax, ranges and names, warn on risky values; exit 4 on errors
    Validate {
        /// File to check (default: the config in use)
        file: Option<PathBuf>,
    },
    /// Print the effective config as TOML, flags applied
    Show,
}

fn parse_severity(s: &str) -> Result<Severity, String> {
    match s.to_ascii_lowercase().as_str() {
        "info" => Ok(Severity::Info),
        "warning" | "warn" => Ok(Severity::Warning),
        "critical" | "crit" => Ok(Severity::Critical),
        _ => Err("expected info, warning or critical".to_string()),
    }
}

fn sev_name(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

/// The same lookup as Config::load: `explicit`, else $XDG_CONFIG_HOME/powerline-monitor/config.toml, else /etc/powerline-monitor.toml.
pub fn config_file(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    let xdg = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/etc"))
        .join("powerline-monitor/config.toml");
    let etc = PathBuf::from("/etc/powerline-monitor.toml");
    if !xdg.exists() && etc.exists() {
        etc
    } else {
        xdg
    }
}

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "localhost".to_string())
}

fn median(v: &[u16]) -> u16 {
    percentile(v, 50.0)
}

/// Nearest-rank percentile of a non-empty slice.
fn percentile(v: &[u16], p: f64) -> u16 {
    let mut s = v.to_vec();
    s.sort_unstable();
    s[((p / 100.0 * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1]
}

struct Row {
    from: Mac,
    to: Mac,
    series: Vec<u16>,
}

impl Row {
    fn now(&self) -> u16 {
        *self.series.last().unwrap()
    }

    fn lowest(&self) -> u16 {
        *self.series.iter().min().unwrap()
    }

    fn json(&self, cfg: &Config) -> serde_json::Value {
        json!({
            "from": self.from, "from_name": cfg.name(self.from), "to": self.to, "to_name": cfg.name(self.to),
            "now": self.now(), "lowest": self.lowest(), "usual": median(&self.series), "readings": self.series.len(),
        })
    }
}

/// The polls at or after `since` and, per directed link, their TX readings, slowest now first.
/// Read from the ring's serialised form, which is also its file and /api/history format.
fn rows(ring: &Ring, since: SystemTime) -> Result<(usize, Vec<Row>)> {
    let polls: Vec<Poll> = serde_json::from_value(serde_json::to_value(ring)?["polls"].take())?;
    let polls: Vec<&Poll> = polls.iter().filter(|p| p.at >= since).collect();
    let mut links: BTreeMap<(Mac, Mac), Vec<u16>> = BTreeMap::new();
    for l in polls.iter().flat_map(|p| &p.links) {
        links.entry((l.from, l.to)).or_default().push(l.tx);
    }
    let mut rows: Vec<Row> = links.into_iter().map(|((from, to), series)| Row { from, to, series }).collect();
    rows.sort_by_key(|r| (r.now(), r.from, r.to));
    Ok((polls.len(), rows))
}

fn load_ring(cfg: &Config) -> Result<Ring> {
    if let Some(url) = &cfg.daemon_url {
        return Client::new(url).history();
    }
    if let Ok(r) = Client::new(&cfg.listen).history() {
        return Ok(r);
    }
    Ring::load(&history_path()).or_else(|_| {
        eprintln!("No history here: the powerline-monitor daemon keeps it where it runs (try --daemon URL or enable the service)");
        exit(3)
    })
}

fn link_table(rows: &[Row], cfg: &Config) -> String {
    let labels: Vec<String> = rows.iter().map(|r| format!("{} → {}", cfg.name(r.from), cfg.name(r.to))).collect();
    let w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(4).max(4);
    let mut out = format!("{:<w$}  {:>4}  {:>6}  {:>5}  {:>8}\n", "LINK", "NOW", "LOWEST", "USUAL", "READINGS");
    for (r, label) in rows.iter().zip(&labels) {
        out.push_str(&format!(
            "{label:<w$}  {:>4}  {:>6}  {:>5}  {:>8}  {}\n",
            r.now(),
            r.lowest(),
            median(&r.series),
            r.series.len(),
            sparkline(&r.series, CELLS)
        ));
    }
    out
}

pub fn history(cfg: &Config, args: &HistoryArgs) -> Result<()> {
    if args.adapters.len() == 1 {
        bail!("history takes either no adapters or exactly two");
    }
    let since = text::parse_since(args.since.as_deref().unwrap_or("24h"))?;
    let ring = load_ring(cfg)?;
    let (readings, mut rows) = rows(&ring, since)?;
    let seen: Vec<Mac> = rows.iter().flat_map(|r| [r.from, r.to]).collect::<BTreeSet<_>>().into_iter().collect();
    let pair = match args.adapters.as_slice() {
        [a, b] => Some((cfg.resolve_adapter(a, &seen)?, cfg.resolve_adapter(b, &seen)?)),
        _ => None,
    };
    if let Some((a, b)) = pair {
        rows.retain(|r| (r.from, r.to) == (a, b) || (r.from, r.to) == (b, a));
    }
    if args.csv {
        print!("{}", ring.to_csv(cfg, pair));
        return Ok(());
    }
    let Some((a, b)) = pair else {
        if args.json {
            println!("{}", serde_json::to_string_pretty(&rows.iter().map(|r| r.json(cfg)).collect::<Vec<_>>())?);
        } else {
            println!("Link speed, Mbit/s, since {} ({})\n", text::stamp(since), text::plural(readings, "reading", "readings"));
            print!("{}", link_table(&rows, cfg));
        }
        return Ok(());
    };
    let now = SystemTime::now();
    let hour = |i: usize| text::hm(now.duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) - (HOURS - i) as f64 * 3600.0);
    let (ab, ba) = (ring.hourly_medians(a, b, HOURS), ring.hourly_medians(b, a, HOURS));
    let lower = |from: Mac, to: Mac, last: Option<u16>| -> Option<(SystemTime, u16, u16)> {
        let p10 = ring.percentile(from, to, 10.0)?;
        let last = last.filter(|l| *l < p10)?;
        let series = ring.series(from, to);
        let since = series.iter().rev().take_while(|s| s.1 < p10).last()?.0;
        Some((since, last, ring.median(from, to)?))
    };
    let verdicts: Vec<(Mac, Mac, SystemTime, u16, u16)> = [(a, b, ab[HOURS - 1]), (b, a, ba[HOURS - 1])]
        .into_iter()
        .filter_map(|(f, t, last)| lower(f, t, last).map(|(s, l, m)| (f, t, s, l, m)))
        .collect();
    if args.json {
        let hourly: Vec<_> = (0..HOURS).map(|i| json!({ "hour": hour(i), "ab": ab[i], "ba": ba[i] })).collect();
        let lower: Vec<_> = verdicts
            .iter()
            .map(|(f, t, s, l, m)| json!({ "from": f, "to": t, "since": text::stamp_full(*s), "now": l, "usual": m }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "links": rows.iter().map(|r| r.json(cfg)).collect::<Vec<_>>(), "hourly": hourly, "lower_than_usual": lower }))?);
        return Ok(());
    }
    println!("Link speed, Mbit/s, since {} ({})\n", text::stamp(since), text::plural(readings, "reading", "readings"));
    print!("{}", link_table(&rows, cfg));
    let (na, nb) = (cfg.name(a), cfg.name(b));
    let w = na.chars().count().max(nb.chars().count()) * 2 + 3;
    let cell = |v: Option<u16>| v.map_or_else(|| "-".to_string(), |v| v.to_string());
    println!("\nHourly usual speed, last {HOURS} hours\n{:<5}  {:>w$}  {:>w$}", "FROM", format!("{na} → {nb}"), format!("{nb} → {na}"));
    for i in 0..HOURS {
        println!("{:<5}  {:>w$}  {:>w$}", hour(i), cell(ab[i]), cell(ba[i]));
    }
    for (f, t, s, l, m) in verdicts {
        println!("\n{} → {}: lower than usual since {} ({l} Mbit/s this hour, usually {m})", cfg.name(f), cfg.name(t), text::stamp(s));
    }
    Ok(())
}

/// Daemon events, else the lines of state_dir/events.jsonl (hook payloads) turned back into events.
fn load_events(cfg: &Config, since: Option<SystemTime>) -> Result<Vec<Event>> {
    if let Some(url) = &cfg.daemon_url {
        return Client::new(url).events(since);
    }
    if let Ok(v) = Client::new(&cfg.listen).events(since) {
        return Ok(v);
    }
    let path = Config::state_dir().join("events.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        eprintln!("No events here: the powerline-monitor daemon records them where it runs (try --daemon URL or enable the service)");
        exit(3)
    };
    Ok(text.lines().filter_map(Event::from_payload).filter(|e| since.is_none_or(|s| e.at() >= s)).collect())
}

pub fn events(cfg: &Config, args: &EventsArgs) -> Result<()> {
    let since = args.since.as_deref().map(text::parse_since).transpose()?;
    let host = hostname();
    let min = args.severity.unwrap_or(Severity::Info);
    let name = |m: Mac| cfg.name(m);
    let mut out = io::stdout().lock();
    let mut last: Option<SystemTime> = None;
    loop {
        let evs = load_events(cfg, last.or(since))?;
        let wanted = |e: &&Event| e.severity() >= min && since.is_none_or(|s| e.at() >= s) && last.is_none_or(|l| e.at() > l);
        for e in evs.iter().filter(wanted) {
            let line = if args.json {
                e.payload(cfg, &host).to_string()
            } else {
                format!("{}  {:<8}  {:<16}  {}", text::stamp(e.at()), sev_name(e.severity()), e.kind(), e.summary(&name))
            };
            if writeln!(out, "{line}").is_err() {
                return Ok(());
            }
        }
        if let Some(e) = evs.last() {
            last = Some(e.at());
        }
        if !args.follow {
            return Ok(());
        }
        out.flush()?;
        thread::sleep(Duration::from_secs(cfg.poll_seconds));
    }
}

/// Resolves against the config alone, then against the adapters seen on one poll when that is not enough.
fn resolve(cfg: &Config, arg: &str) -> Result<Mac> {
    cfg.resolve_adapter(arg, &[]).or_else(|e| match poll_once(cfg) {
        Ok(s) if !s.devices.is_empty() => cfg.resolve_adapter(arg, &s.macs()),
        _ => Err(e),
    })
}

/// Resolves with the effective config (flags applied) but writes from `file_cfg`, so -i and --daemon are not persisted.
pub fn rename(cfg: &Config, file_cfg: &Config, path: &Path, args: &RenameArgs) -> Result<()> {
    let mac = resolve(cfg, &args.adapter)?;
    let mut next = file_cfg.clone();
    next.devices.entry(mac).or_default().alias = args.name.clone();
    Config::parse(&toml::to_string(&next)?)?;
    next.save(path)?;
    match &args.name {
        Some(n) => println!("Named {mac} \"{n}\" in {}. This is a local name; the adapter itself is not written.", path.display()),
        None => println!("Cleared the name of {mac} in {}; it shows as \"{}\".", path.display(), next.name(mac)),
    }
    Ok(())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// "unknown field `pol_seconds`, expected one of `a`, `b`" -> "unknown field `pol_seconds`; did you mean `poll_seconds`?"
fn suggest(msg: &str) -> String {
    let Some((head, tail)) = msg.split_once(", expected") else { return msg.to_string() };
    let Some(field) = head.strip_prefix("unknown field `").and_then(|s| s.strip_suffix('`')) else { return msg.to_string() };
    let best = tail
        .split('`')
        .skip(1)
        .step_by(2)
        .map(|k| (levenshtein(field, k), k))
        .min()
        .filter(|(d, _)| *d <= 3)
        .map(|(_, k)| k);
    match best {
        Some(k) => format!("{head}; did you mean `{k}`?"),
        None => msg.to_string(),
    }
}

fn executable(cmd: &str) -> bool {
    let is_exec = |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if cmd.contains('/') {
        return is_exec(Path::new(cmd));
    }
    env::var_os("PATH").is_some_and(|p| env::split_paths(&p).any(|d| is_exec(&d.join(cmd))))
}

/// Prints errors and warnings for the file; returns the exit code (0, or 4 on errors).
pub fn validate(path: &Path) -> i32 {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", path.display());
            return 4;
        }
    };
    let cfg = match Config::parse(&text) {
        Ok(c) => c,
        Err(e) => {
            match e.downcast_ref::<toml::de::Error>() {
                Some(te) => {
                    let line = te.span().map_or(0, |s| text[..s.start.min(text.len())].matches('\n').count() + 1);
                    eprintln!("error: {}:{line}: {}", path.display(), suggest(te.message()));
                }
                None => eprintln!("error: {}: {e}", path.display()),
            }
            return 4;
        }
    };
    if let Ok(addr) = cfg.listen.parse::<SocketAddr>() {
        if !addr.ip().is_loopback() {
            eprintln!("warning: listen {} is not loopback; /metrics and /api have no authentication", cfg.listen);
        }
    }
    for i in cfg.interfaces.iter().filter(|i| !Path::new("/sys/class/net").join(i).exists()) {
        eprintln!("warning: interface {i} is not present on this computer");
    }
    if let Some(cmd) = cfg.hooks.exec.first().filter(|c| !executable(c)) {
        eprintln!("warning: hooks.exec[0] {cmd:?} is not an executable file");
    }
    println!(
        "{} adapters named, {} spares · poll {} s · history {} h · metrics on {} · hook {}",
        cfg.devices.len(),
        cfg.spares.len(),
        cfg.poll_seconds,
        cfg.history_hours,
        cfg.listen,
        if cfg.hooks.exec.is_empty() { "no" } else { "yes" }
    );
    0
}

pub fn show_config(cfg: &Config) -> Result<()> {
    print!("{}", toml::to_string_pretty(cfg)?);
    Ok(())
}

pub fn export(cfg: &Config) -> Result<()> {
    let t0 = Instant::now();
    let net = Poller::new(cfg).poll(cfg)?;
    let meta = PollMeta { last_poll_ok: true, polls: 1, last_poll_at: Some(net.at), poll_duration: t0.elapsed().as_secs_f64() };
    print!("{}", powerline_daemon::metrics(Some(&net), cfg, &BTreeMap::new(), &meta));
    Ok(())
}
