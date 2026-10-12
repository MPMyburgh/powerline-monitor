use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Write};
use std::process::exit;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, execute, queue};
use powerline_core::client::Client;
use powerline_core::config::Config;
use powerline_core::history::Ring;
use powerline_core::model::{Mac, NetState};
use powerline_core::poll::Poller;
use powerline_core::render::{self, sparkline};
use powerline_core::text;

#[derive(clap::Args, Debug, Clone, Default)]
pub struct Args {
    /// Seconds between polls (default 5; the config's poll_seconds applies to the daemon)
    #[arg(long, default_value_t = 5, hide_default_value = true, value_name = "N")]
    pub interval_seconds: u64,
    /// One line per poll instead of the full-screen view (automatic when stdout is not a terminal)
    #[arg(long)]
    pub lines: bool,
    /// One compact JSON NetState per poll
    #[arg(long)]
    pub json_lines: bool,
}

enum Key {
    Quit,
    PollNow,
    Tick,
    Redraw,
}

struct Term(io::Stdout);

impl Term {
    fn enter() -> Result<Term> {
        let mut out = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Term(out))
    }

    fn draw(&mut self, text: &str) -> Result<()> {
        queue!(self.0, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0), Print(text.replace('\n', "\r\n")))?;
        self.0.flush()?;
        Ok(())
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        execute!(self.0, cursor::Show, terminal::LeaveAlternateScreen).ok();
        terminal::disable_raw_mode().ok();
    }
}

pub fn run(cfg: &Config, args: &Args) -> Result<()> {
    let mut secs = args.interval_seconds.clamp(1, 600);
    let mut poller = Poller::new(cfg);
    let client = cfg.daemon_url.as_deref().map(Client::new);
    let mut poll = || match &client {
        Some(c) => c.state()?.ok_or_else(|| anyhow!("daemon at {} has not polled yet", c.url())),
        None => poller.poll(cfg),
    };
    let mut state = poll()?;
    if state.devices.is_empty() {
        eprintln!("no adapters found on {}", cfg.effective_interfaces().join(", "));
        exit(3);
    }
    if args.json_lines || args.lines || !io::stdout().is_terminal() {
        let mut out = io::stdout().lock();
        loop {
            let text = if args.json_lines { serde_json::to_string(&state)? } else { line(&state, cfg) };
            if writeln!(out, "{text}").and_then(|_| out.flush()).is_err() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(secs));
            match poll() {
                Ok(s) => state = s,
                Err(e) => {
                    if writeln!(out, "{}  poll failed: {e:#}", text::stamp_seconds(SystemTime::now())).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    let mut ring = Ring::new((cfg.history_hours * 3600 / secs) as usize);
    ring.push(&state);
    let started = SystemTime::now();
    let mut term = Term::enter()?;
    let mut polls = 1;
    let mut err: Option<String> = None;
    let mut paused = false;
    let mut offline: BTreeMap<Mac, SystemTime> = BTreeMap::new();
    loop {
        let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
        let status = match (&err, paused) {
            (_, true) => "paused".to_string(),
            (Some(e), _) => format!("poll failed: {e}"),
            (None, _) => "ok".to_string(),
        };
        let header = format!(
            "powerline-monitor watch · every {secs} s · since {} ({}) · {status} · q quit  r poll now  p pause  +/- interval\n\n",
            text::stamp(started),
            text::plural(polls, "reading", "readings")
        );
        term.draw(&format!("{header}{}", screen(&state, &ring, cfg, &offline, width)))?;
        match wait(Duration::from_secs(secs), &mut secs, &mut paused)? {
            Key::Quit => return Ok(()),
            Key::Redraw => continue,
            Key::Tick if paused => continue,
            Key::Tick | Key::PollNow => {}
        }
        match poll() {
            Ok(s) => {
                for m in state.macs().into_iter().filter(|m| !s.devices.iter().any(|d| d.mac == *m)) {
                    offline.entry(m).or_insert(s.at);
                }
                for d in &s.devices {
                    offline.remove(&d.mac);
                }
                ring.push(&s);
                state = s;
                polls += 1;
                err = None;
            }
            Err(e) => err = Some(format!("{e:#}")),
        }
    }
}

/// Sleeps `interval` unless a key arrives: q/Ctrl-C quit, r polls now, p toggles pause, +/- move the interval by 5 s.
fn wait(interval: Duration, secs: &mut u64, paused: &mut bool) -> Result<Key> {
    let until = Instant::now() + interval;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Key::Tick);
        }
        if event::poll(left)? {
            if let Event::Key(k) = event::read()? {
                let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
                match k.code {
                    _ if ctrl_c => return Ok(Key::Quit),
                    KeyCode::Char('q') => return Ok(Key::Quit),
                    KeyCode::Char('r') => return Ok(Key::PollNow),
                    KeyCode::Char('p') => {
                        *paused = !*paused;
                        return Ok(Key::Redraw);
                    }
                    KeyCode::Char('+') => {
                        *secs = (*secs + 5).min(600);
                        return Ok(Key::Redraw);
                    }
                    KeyCode::Char('-') => {
                        *secs = secs.saturating_sub(5).max(1);
                        return Ok(Key::Redraw);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The name cut to 8 characters; a default "Adapter 1B2C" keeps just its hex suffix.
fn short(cfg: &Config, mac: Mac) -> String {
    let name = cfg.name(mac);
    match name.strip_prefix("Adapter ").filter(|r| r.len() == 4) {
        Some(suffix) => suffix.to_string(),
        None => name.chars().take(8).collect(),
    }
}

/// One line per poll: the time, then every directed link as `From>To rate`.
fn line(state: &NetState, cfg: &Config) -> String {
    let mut out = text::stamp_seconds(state.at);
    for from in state.macs() {
        let Some(s) = state.snapshot(from) else { continue };
        for l in &s.links {
            write!(out, "  {}>{} {}", short(cfg, from), short(cfg, l.peer), l.tx_mbps).unwrap();
        }
    }
    out
}

fn screen(state: &NetState, ring: &Ring, cfg: &Config, offline: &BTreeMap<Mac, SystemTime>, width: usize) -> String {
    let mut out = render::status_text(state, cfg, None, false);
    writeln!(out, "\nLink speed, Mbit/s · lowest and usual since start").unwrap();
    let pairs = render::pairs(state);
    let labels: Vec<String> = pairs.iter().map(|(a, b, _, _)| format!("{} ↔ {}", cfg.name(*a), cfg.name(*b))).collect();
    let w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    for (i, ((a, b, ab, ba), label)) in pairs.iter().zip(&labels).enumerate() {
        let (from, to) = if ab <= ba { (*a, *b) } else { (*b, *a) };
        let lowest = [ring.stats(*a, *b), ring.stats(*b, *a)].iter().flatten().map(|s| s.min).min().unwrap_or(0);
        let usual = ring.stats(from, to).map_or(0, |s| s.median);
        let prefix = format!("{label:<w$}  {ab:>4} / {ba:<4}  lowest {lowest:>4}  usual {usual:>4}  ");
        let tx: Vec<u16> = ring.series(from, to).iter().map(|s| s.1).collect();
        let cells = width.saturating_sub(prefix.chars().count() + 10).max(8);
        let mark = if i == 0 { " ← slowest" } else { "" };
        writeln!(out, "{prefix}{}{mark}", sparkline(&tx, cells)).unwrap();
    }
    for (mac, since) in offline {
        writeln!(out, "{}  offline since {}", cfg.name(*mac), text::stamp(*since)).unwrap();
    }
    writeln!(out, "▁ <75  ▄ 300  █ 525+ Mbit/s").unwrap();
    out
}
