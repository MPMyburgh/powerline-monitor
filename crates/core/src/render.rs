use std::collections::BTreeSet;
use std::fmt::Write;

use serde::Serialize;

use crate::config::Config;
use crate::events::{Severity, Tracker};
use crate::model::{human_uptime, Mac, NetState};
use crate::standard::signal_level_text;
use crate::text;

pub const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Clone, Debug, Serialize)]
pub struct Verdict {
    pub severity: Option<Severity>,
    pub line: String,
    pub title: String,
}

fn table(rows: &[Vec<String>]) -> String {
    let cols = rows[0].len();
    let widths: Vec<usize> = (0..cols).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0)).collect();
    let mut out = String::new();
    for r in rows {
        let line: Vec<String> = r.iter().enumerate().map(|(c, v)| format!("{:<w$}", v, w = widths[c])).collect();
        writeln!(out, "{}", line.join("  ").trim_end()).unwrap();
    }
    out
}

/// The adapters seen this poll plus the configured ones that did not show up.
fn roster(state: &NetState, cfg: &Config) -> Vec<Mac> {
    let mut v = state.macs();
    let seen: BTreeSet<Mac> = v.iter().copied().collect();
    v.extend(cfg.devices.keys().filter(|m| !seen.contains(m)));
    v
}

/// Answering adapters out of the roster.
fn online(state: &NetState, cfg: &Config) -> (usize, usize) {
    let r = roster(state, cfg);
    (r.iter().filter(|m| state.snapshot(**m).is_some()).count(), r.len())
}

/// Transmitter's view of from -> to, else the receiver's view (flagged true).
fn directed(state: &NetState, from: Mac, to: Mac) -> Option<(u16, bool)> {
    state.rate(from, to).map(|(tx, _)| (tx, false)).or_else(|| state.rate(to, from).map(|(_, rx)| (rx, true)))
}

/// Unordered pairs (a, b, a->b, b->a), slowest first.
pub fn pairs(state: &NetState) -> Vec<(Mac, Mac, u16, u16)> {
    let macs = state.macs();
    let mut v = Vec::new();
    for (i, a) in macs.iter().enumerate() {
        for b in &macs[i + 1..] {
            if let (Some((ab, _)), Some((ba, _))) = (directed(state, *a, *b), directed(state, *b, *a)) {
                v.push((*a, *b, ab, ba));
            }
        }
    }
    v.sort_by_key(|p| (p.2.min(p.3), p.0, p.1));
    v
}

/// Every directed link with its current rate, slowest first.
fn links(state: &NetState) -> Vec<(Mac, Mac, u16)> {
    let mut v: Vec<(Mac, Mac, u16)> =
        state.snapshots.iter().flat_map(|s| s.links.iter().map(move |l| (s.device, l.peer, l.tx_mbps))).collect();
    v.sort_by_key(|l| (l.2, l.0, l.1));
    v
}

fn degraded(tracker: Option<&Tracker>) -> BTreeSet<(Mac, Mac)> {
    tracker.map(|t| t.degraded().clone()).unwrap_or_default()
}

fn other_networks(state: &NetState) -> usize {
    let own = state.nid();
    let nids: BTreeSet<_> =
        state.snapshots.iter().flat_map(|s| s.neighbour_nets.iter().map(|n| n.nid)).filter(|n| Some(*n) != own).collect();
    nids.len()
}

pub fn verdict(state: &NetState, cfg: &Config, tracker: Option<&Tracker>) -> Verdict {
    let v = |severity, line: String, title: &str| Verdict { severity, line, title: title.to_string() };
    if !cfg.devices.is_empty() {
        if let Some(m) = state.macs().into_iter().find(|m| !cfg.is_owned(*m)) {
            let line = format!("CRITICAL  An adapter that is not yours ({m}) is on your network.");
            return v(Some(Severity::Critical), line, "Powerline: unknown adapter");
        }
    }
    let (n, m) = online(state, cfg);
    let floor = cfg.alerts.floor_mbps;
    let degraded = degraded(tracker);
    let slow = links(state).into_iter().find(|(a, b, rate)| *rate < floor || degraded.contains(&(*a, *b)));
    if state.default_key() {
        let links = match slow {
            Some((a, b, rate)) => format!("{} → {} slow at {rate} Mbit/s", cfg.name(a), cfg.name(b)),
            None => "links normal".to_string(),
        };
        let line = format!("CRITICAL  Factory password in use. {n} of {m} adapters online, {links}.");
        return v(Some(Severity::Critical), line, "Powerline: factory password");
    }
    let missing: Vec<String> = cfg.devices.keys().filter(|m| state.snapshot(**m).is_none()).map(|m| cfg.name(*m)).collect();
    if !missing.is_empty() {
        let verb = if missing.len() == 1 { "has" } else { "have" };
        let title = if missing.len() == 1 { format!("Powerline: {} offline", missing[0]) } else { format!("Powerline: {} adapters offline", missing.len()) };
        return v(Some(Severity::Warning), format!("WARNING  {} {verb} not answered.", missing.join(", ")), &title);
    }
    if let Some((a, b, rate)) = slow {
        let line = format!("WARNING  {} → {} is slow: {rate} Mbit/s.", cfg.name(a), cfg.name(b));
        return v(Some(Severity::Warning), line, "Powerline: slow link");
    }
    let slowest = match links(state).first() {
        Some((a, b, rate)) => format!(", slowest link {} → {} {rate} Mbit/s", cfg.name(*a), cfg.name(*b)),
        None => String::new(),
    };
    v(None, format!("OK  Powerline is fine: {n} of {m} adapters online{slowest}"), "Powerline: OK")
}

pub fn status_text(state: &NetState, cfg: &Config, tracker: Option<&Tracker>, verbose: bool) -> String {
    let mut out = String::new();
    writeln!(out, "{}\n", verdict(state, cfg, tracker).line).unwrap();
    let roster = roster(state, cfg);
    let who = |m: Option<Mac>| m.map_or_else(|| "none".to_string(), |m| cfg.name(m));
    write!(out, "Network: {} adapters · coordinator {} · backup {}", roster.len(), who(state.cco()), who(state.bcco())).unwrap();
    if verbose {
        if let Some(s) = state.nid().and_then(|nid| state.snapshots.iter().find(|s| s.nid == nid)) {
            write!(out, " · NID {} · SNID {} · {}", s.nid, s.snid, s.nid.security_level()).unwrap();
        }
    }
    out.push('\n');
    let observer = state.devices.iter().find(|d| d.local).and_then(|d| state.snapshot(d.mac)).or(state.snapshots.first());
    let mut header = vec!["NAME", "ROLE", "WHERE", "FIRMWARE", "UP"];
    if verbose {
        header.extend(["MAC", "TEI", "MII", "SIGNAL", "LINK"]);
    }
    let mut rows = vec![header.into_iter().map(String::from).collect::<Vec<_>>()];
    for mac in &roster {
        let dev = state.devices.iter().find(|d| d.mac == *mac);
        let snap = state.snapshot(*mac);
        let dash = || "-".to_string();
        let where_ = match dev {
            Some(d) if d.local => "wired to this computer",
            Some(d) if d.seen_by_peers_only => "seen by peers, not answering",
            Some(_) => "over the powerline",
            None => "not answering",
        };
        let firmware = snap
            .and_then(|s| s.firmware.clone())
            .or_else(|| dev.and_then(|d| d.hfid.as_ref()).map(|h| format!("tag {h}")))
            .unwrap_or_else(dash);
        let mut row = vec![
            cfg.name(*mac),
            snap.map_or_else(dash, |s| s.role.human().to_lowercase()),
            where_.to_string(),
            firmware,
            snap.and_then(|s| s.uptime_secs).map_or_else(dash, human_uptime),
        ];
        if verbose {
            let me = observer.is_some_and(|o| o.device == *mac);
            let signal = observer
                .filter(|_| !me)
                .and_then(|o| o.heard.iter().find(|h| h.mac == *mac))
                .map_or_else(dash, |h| signal_level_text(h.signal_level));
            let link = observer
                .filter(|_| !me)
                .and_then(|o| o.links.iter().find(|l| l.peer == *mac))
                .map_or_else(dash, |l| l.tx_flags.label());
            row.extend([
                mac.to_string(),
                snap.map_or_else(dash, |s| s.tei.to_string()),
                dev.map_or_else(dash, |d| d.iface.clone()),
                signal,
                link,
            ]);
        }
        rows.push(row);
    }
    out.push_str(&table(&rows));

    let floor = cfg.alerts.floor_mbps;
    let macs: Vec<Mac> = state.macs().into_iter().filter(|m| state.snapshot(*m).is_some()).collect();
    if macs.len() > 1 {
        let labels: Vec<String> = macs.iter().map(|m| cfg.name(*m)).collect();
        let w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(7) + 2;
        writeln!(out, "\nLink speed, Mbit/s (row sends to column; real throughput is roughly half)").unwrap();
        write!(out, "{:w$}", "").unwrap();
        for l in &labels {
            write!(out, "{l:>w$}").unwrap();
        }
        out.push('\n');
        for (i, from) in macs.iter().enumerate() {
            write!(out, "{:<w$}", labels[i]).unwrap();
            for to in &macs {
                let cell = if from == to {
                    "-".to_string()
                } else {
                    match directed(state, *from, *to) {
                        Some((rate, from_rx)) => {
                            format!("{rate}{}{}", if from_rx { "~" } else { "" }, if rate < floor { "!" } else { "" })
                        }
                        None => "?".to_string(),
                    }
                };
                write!(out, "{cell:>w$}").unwrap();
            }
            out.push('\n');
        }
        let pairs = pairs(state);
        if !pairs.is_empty() {
            writeln!(out, "\nLINKS, slowest first").unwrap();
            let rows: Vec<Vec<String>> = pairs
                .iter()
                .map(|(a, b, ab, ba)| {
                    let grade = if ab.min(ba) < &floor { "poor !" } else { "ok" };
                    vec![format!("{} ↔ {}", cfg.name(*a), cfg.name(*b)), format!("{ab} / {ba}"), grade.to_string()]
                })
                .collect();
            out.push_str(&table(&rows));
        }
    }

    let mut footer = Vec::new();
    if state.default_key() {
        footer.push("Factory password in use: see powerline-monitor check-default-key".to_string());
    }
    let others = other_networks(state);
    if others > 0 {
        footer.push(format!("Other networks heard on the wiring: {others}"));
    }
    if cfg.devices.is_empty() {
        footer.push("No adapters named in the config, so unknown-adapter detection is off".to_string());
    }
    if !footer.is_empty() {
        out.push('\n');
        for line in footer {
            writeln!(out, "{line}").unwrap();
        }
    }
    out
}

pub fn discover_table(state: &NetState, cfg: &Config) -> String {
    let mut out = String::new();
    let mut ifaces: Vec<&str> = state.devices.iter().map(|d| d.iface.as_str()).collect();
    ifaces.dedup();
    for iface in ifaces {
        let devs: Vec<_> = state.macs().into_iter().filter_map(|m| state.devices.iter().find(|d| d.mac == m && d.iface == iface)).collect();
        let wired = devs.iter().filter(|d| d.local).count();
        writeln!(
            out,
            "{iface}: {} wired to this computer, {} more over the powerline",
            text::plural(wired, "adapter", "adapters"),
            devs.len() - wired
        )
        .unwrap();
        let mut rows = vec![["NAME", "MAC", "WHERE", "ROLE", "FIRMWARE"].map(String::from).to_vec()];
        for d in devs {
            let snap = state.snapshot(d.mac);
            let where_ = if d.local {
                "wired here"
            } else if d.seen_by_peers_only {
                "powerline, not answering"
            } else {
                "powerline"
            };
            rows.push(vec![
                cfg.name(d.mac),
                d.mac.to_string(),
                where_.to_string(),
                snap.map_or_else(|| "-".to_string(), |s| s.role.human().to_lowercase()),
                snap.and_then(|s| s.firmware.clone())
                    .or_else(|| d.hfid.as_ref().map(|h| format!("tag {h}")))
                    .unwrap_or_else(|| "-".to_string()),
            ]);
        }
        out.push_str(&table(&rows));
    }
    if let Some(nid) = state.nid() {
        let unnamed = state.devices.iter().filter(|d| !cfg.is_owned(d.mac)).count();
        let named = if unnamed == 0 { "all named in the config".to_string() } else { format!("{unnamed} not named") };
        writeln!(out, "\nNetwork {nid}: {} adapters, {named}.", state.devices.len()).unwrap();
    }
    if state.default_key() {
        writeln!(out, "Factory password in use: see powerline-monitor check-default-key").unwrap();
    }
    out
}

/// One `[devices]` block per adapter not yet in the config, with a commented alias line, ready to append.
pub fn discover_toml(state: &NetState, cfg: &Config) -> String {
    const ROOMS: [&str; 4] = ["Lounge", "Study", "Bedroom", "Kitchen"];
    let mut out = String::new();
    for (i, mac) in state.macs().into_iter().filter(|m| !cfg.is_owned(*m)).enumerate() {
        let dev = state.devices.iter().find(|d| d.mac == mac).unwrap();
        let mut notes = vec![if dev.local { "wired here" } else { "powerline" }];
        let role = state.snapshot(mac).map(|s| s.role.human().to_lowercase());
        if let Some(r) = &role {
            notes.push(r);
        }
        writeln!(out, "\n[devices.\"{mac}\"]\n#alias = \"{}\"  # {}", ROOMS[i % ROOMS.len()], notes.join(", ")).unwrap();
    }
    out
}

/// Lowest value per bucket on an absolute scale of 75 Mbit/s per block, the whole series squeezed into `cells`.
pub fn sparkline(series: &[u16], cells: usize) -> String {
    if series.is_empty() {
        return String::new();
    }
    let n = series.len();
    let cells = cells.min(n).max(1);
    (0..cells)
        .map(|c| {
            let lo = c * n / cells;
            let hi = ((c + 1) * n / cells).max(lo + 1);
            let min = series[lo..hi].iter().min().copied().unwrap_or(0);
            BLOCKS[(min as usize / 75).min(7)]
        })
        .collect()
}
