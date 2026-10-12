use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::SystemTime;

use crate::config::Config;
use crate::history::Ring;
use crate::keys;
use crate::model::{human_uptime, rfc3339, Mac, NetState, Nid};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    #[serde(rename = "adapter.seen")]
    AdapterSeen { #[serde(with = "rfc3339")] at: SystemTime, mac: Mac },
    #[serde(rename = "adapter.lost")]
    AdapterLost { #[serde(with = "rfc3339")] at: SystemTime, mac: Mac },
    #[serde(rename = "cco.changed")]
    CcoChanged { #[serde(with = "rfc3339")] at: SystemTime, was: Option<Mac>, now: Option<Mac> },
    #[serde(rename = "nmk.changed")]
    NidChanged { #[serde(with = "rfc3339")] at: SystemTime, was: Nid, now: Nid },
    #[serde(rename = "link.degraded")]
    RateDegraded { #[serde(with = "rfc3339")] at: SystemTime, from: Mac, to: Mac, was: u16, now: u16 },
    #[serde(rename = "link.recovered")]
    RateRecovered { #[serde(with = "rfc3339")] at: SystemTime, from: Mac, to: Mac, now: u16, lowest: u16, after_secs: u64 },
    #[serde(rename = "station.unknown")]
    UnknownAdapterJoined { #[serde(with = "rfc3339")] at: SystemTime, mac: Mac },
    #[serde(rename = "station.gone")]
    UnknownAdapterGone { #[serde(with = "rfc3339")] at: SystemTime, mac: Mac },
    #[serde(rename = "nmk.default")]
    DefaultKeyDetected { #[serde(with = "rfc3339")] at: SystemTime, nid: Nid },
    #[serde(rename = "poller.blind")]
    PollFailed { #[serde(with = "rfc3339")] at: SystemTime, error: String },
    #[serde(rename = "poller.ok")]
    PollerOk { #[serde(with = "rfc3339")] at: SystemTime, after_secs: u64 },
    #[serde(rename = "adapter.rebooted")]
    AdapterRebooted { #[serde(with = "rfc3339")] at: SystemTime, mac: Mac, was_up_secs: u32, now_up_secs: u32 },
    #[serde(rename = "network.heard")]
    NetworkHeard { #[serde(with = "rfc3339")] at: SystemTime, nid: Nid, default_key: bool, own_default: bool },
}

fn duration(secs: u64) -> String {
    human_uptime(secs.min(u32::MAX as u64) as u32)
}

impl Event {
    pub fn at(&self) -> SystemTime {
        match self {
            Event::AdapterSeen { at, .. }
            | Event::AdapterLost { at, .. }
            | Event::CcoChanged { at, .. }
            | Event::NidChanged { at, .. }
            | Event::RateDegraded { at, .. }
            | Event::RateRecovered { at, .. }
            | Event::UnknownAdapterJoined { at, .. }
            | Event::UnknownAdapterGone { at, .. }
            | Event::DefaultKeyDetected { at, .. }
            | Event::PollFailed { at, .. }
            | Event::PollerOk { at, .. }
            | Event::AdapterRebooted { at, .. }
            | Event::NetworkHeard { at, .. } => *at,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Event::AdapterSeen { .. } => "adapter.seen",
            Event::AdapterLost { .. } => "adapter.lost",
            Event::CcoChanged { .. } => "cco.changed",
            Event::NidChanged { .. } => "nmk.changed",
            Event::RateDegraded { .. } => "link.degraded",
            Event::RateRecovered { .. } => "link.recovered",
            Event::UnknownAdapterJoined { .. } => "station.unknown",
            Event::UnknownAdapterGone { .. } => "station.gone",
            Event::DefaultKeyDetected { .. } => "nmk.default",
            Event::PollFailed { .. } => "poller.blind",
            Event::PollerOk { .. } => "poller.ok",
            Event::AdapterRebooted { .. } => "adapter.rebooted",
            Event::NetworkHeard { .. } => "network.heard",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Event::AdapterSeen { .. }
            | Event::RateRecovered { .. }
            | Event::UnknownAdapterGone { .. }
            | Event::PollerOk { .. } => Severity::Info,
            Event::AdapterLost { .. }
            | Event::CcoChanged { .. }
            | Event::NidChanged { .. }
            | Event::RateDegraded { .. }
            | Event::PollFailed { .. }
            | Event::AdapterRebooted { .. } => Severity::Warning,
            Event::NetworkHeard { default_key, own_default, .. } => {
                if *default_key && !*own_default {
                    Severity::Critical
                } else {
                    Severity::Warning
                }
            }
            Event::UnknownAdapterJoined { .. } | Event::DefaultKeyDetected { .. } => Severity::Critical,
        }
    }

    pub fn summary(&self, name: &dyn Fn(Mac) -> String) -> String {
        match self {
            Event::AdapterSeen { mac, .. } => format!("{} is back online", name(*mac)),
            Event::AdapterLost { mac, .. } => format!("{} has gone offline (not answering)", name(*mac)),
            Event::CcoChanged { was, now, .. } => {
                let n = |m: &Option<Mac>| m.map(name).unwrap_or_else(|| "nobody".to_string());
                format!("{} took over coordinating the network from {}", n(now), n(was))
            }
            Event::NidChanged { .. } => "The powerline network password changed".to_string(),
            Event::DefaultKeyDetected { .. } => {
                "The powerline network is on the factory password; anyone's adapter on this wiring can join".to_string()
            }
            Event::RateDegraded { from, to, was, now, .. } => {
                format!("{} → {} is slow: {now} Mbit/s, was {was}", name(*from), name(*to))
            }
            Event::RateRecovered { from, to, now, after_secs, .. } => {
                format!("{} → {} is back to {now} Mbit/s after {}", name(*from), name(*to), duration(*after_secs))
            }
            Event::UnknownAdapterJoined { mac, .. } => format!("An adapter that is not yours ({mac}) joined your network"),
            Event::UnknownAdapterGone { mac, .. } => format!("The unknown adapter {mac} left"),
            Event::PollFailed { error, .. } => format!("Can't reach the adapters: {error}"),
            Event::PollerOk { after_secs, .. } => format!("Adapters answering again after {}", duration(*after_secs)),
            Event::AdapterRebooted { mac, was_up_secs, now_up_secs, .. } => format!(
                "{} restarted {} ago (was up {})",
                name(*mac),
                human_uptime(*now_up_secs),
                human_uptime(*was_up_secs)
            ),
            Event::NetworkHeard { default_key, .. } => format!(
                "Another powerline network is on this wiring{}",
                if *default_key { " (factory password)" } else { "" }
            ),
        }
    }

    pub fn advice(&self) -> Option<&'static str> {
        match self {
            Event::AdapterLost { .. } => {
                Some("Check it is plugged straight into a wall socket, not a power strip, and that its Power light is on")
            }
            Event::CcoChanged { .. } => Some("Nothing to do unless it keeps happening"),
            Event::NidChanged { .. } => Some("Expected after a rekey or pairing; if nobody did either, find out who did"),
            Event::DefaultKeyDetected { .. } => Some("Fix it in a terminal: powerline-monitor rekey --dry-run first"),
            Event::RateDegraded { .. } => Some(
                "Something new plugged in near either adapter? Chargers, heaters, washing machines and power strips add noise",
            ),
            Event::UnknownAdapterJoined { .. } => {
                Some("If nobody in the house just plugged in a new powerline adapter, act now")
            }
            Event::PollFailed { .. } => Some("Check this computer's cable to the adapter"),
            _ => None,
        }
    }

    /// The hook/JSONL record: time, host, type, severity, subject, summary, advice, detail (variant fields), nid.
    pub fn payload(&self, cfg: &Config, host: &str) -> Value {
        let name = |m: Mac| cfg.name(m);
        let subject = match self {
            Event::AdapterSeen { mac, .. }
            | Event::AdapterLost { mac, .. }
            | Event::UnknownAdapterJoined { mac, .. }
            | Event::UnknownAdapterGone { mac, .. }
            | Event::AdapterRebooted { mac, .. } => json!({ "mac": mac, "name": name(*mac) }),
            Event::CcoChanged { now: Some(mac), .. } => json!({ "mac": mac, "name": name(*mac) }),
            Event::RateDegraded { from, to, .. } | Event::RateRecovered { from, to, .. } => {
                json!({ "tx": from, "tx_name": name(*from), "rx": to, "rx_name": name(*to) })
            }
            _ => Value::Null,
        };
        let nid = match self {
            Event::NidChanged { now, .. } => Some(*now),
            Event::DefaultKeyDetected { nid, .. } | Event::NetworkHeard { nid, .. } => Some(*nid),
            _ => None,
        };
        let mut detail = serde_json::to_value(self).unwrap();
        if let Some(d) = detail.as_object_mut() {
            d.remove("kind");
            d.remove("at");
        }
        json!({
            "time": rfc3339::format(self.at()),
            "host": host,
            "type": self.kind(),
            "severity": self.severity(),
            "subject": subject,
            "summary": self.summary(&name),
            "advice": self.advice(),
            "detail": detail,
            "nid": nid,
        })
    }
}

impl Event {
    /// The event behind one payload() line of events.jsonl; None for a line that is not one.
    pub fn from_payload(line: &str) -> Option<Event> {
        let mut v: Value = serde_json::from_str(line).ok()?;
        let mut ev = v["detail"].take();
        ev["kind"] = v["type"].take();
        ev["at"] = v["time"].take();
        serde_json::from_value(ev).ok()
    }
}

fn opt(m: &Option<Mac>) -> String {
    m.map(|m| m.to_string()).unwrap_or_else(|| "none".to_string())
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::AdapterSeen { mac, .. } => write!(f, "adapter {mac} seen"),
            Event::AdapterLost { mac, .. } => write!(f, "adapter {mac} lost"),
            Event::CcoChanged { was, now, .. } => write!(f, "CCo changed from {} to {}", opt(was), opt(now)),
            Event::NidChanged { was, now, .. } => write!(f, "NID changed from {was} to {now}"),
            Event::RateDegraded { from, to, was, now, .. } => {
                write!(f, "{from} -> {to} degraded: {was} -> {now} Mbit/s")
            }
            Event::RateRecovered { from, to, now, lowest, after_secs, .. } => {
                write!(f, "{from} -> {to} recovered: {now} Mbit/s (lowest {lowest}) after {after_secs} s")
            }
            Event::UnknownAdapterJoined { mac, .. } => write!(f, "unknown adapter {mac} joined (not in config)"),
            Event::UnknownAdapterGone { mac, .. } => write!(f, "unknown adapter {mac} gone"),
            Event::DefaultKeyDetected { nid, .. } => write!(f, "network {nid} uses the default key \"HomePlugAV\""),
            Event::PollFailed { error, .. } => write!(f, "poll failed: {error}"),
            Event::PollerOk { after_secs, .. } => write!(f, "poll ok again after {after_secs} s"),
            Event::AdapterRebooted { mac, was_up_secs, now_up_secs, .. } => {
                write!(f, "adapter {mac} rebooted: was up {was_up_secs} s, now up {now_up_secs} s")
            }
            Event::NetworkHeard { nid, default_key, .. } => {
                write!(f, "another network {nid} heard{}", if *default_key { " (default key)" } else { "" })
            }
        }
    }
}

/// serde for maps whose keys are not strings: stored as a sequence of (key, value) pairs.
mod pairs {
    use super::*;

    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(m: &BTreeMap<K, V>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter())
    }

    pub fn deserialize<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<K, V>, D::Error> {
        Ok(Vec::<(K, V)>::deserialize(d)?.into_iter().collect())
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Tracker {
    prev: Option<NetState>,
    missing: BTreeMap<Mac, u32>,
    #[serde(with = "pairs")]
    below: BTreeMap<(Mac, Mac), (u32, u16, SystemTime)>,
    #[serde(with = "pairs")]
    above: BTreeMap<(Mac, Mac), u32>,
    degraded: BTreeSet<(Mac, Mac)>,
    blind_polls: u32,
    #[serde(with = "rfc3339::option")]
    blind_since: Option<SystemTime>,
    unknown: BTreeMap<Mac, u32>,
    heard: BTreeSet<Nid>,
}

impl Tracker {
    pub fn degraded(&self) -> &BTreeSet<(Mac, Mac)> {
        &self.degraded
    }

    /// Feeds one poll result and returns the events it produced. The first successful poll reports only
    /// the factory key, unknown stations and foreign networks.
    pub fn observe(&mut self, result: Result<&NetState, &str>, ring: &Ring, cfg: &Config) -> Vec<Event> {
        let mut ev = Vec::new();
        let lost_after = cfg.alerts.lost_after_polls.max(1);
        let degraded_after = cfg.alerts.degraded_after_polls.max(1);
        let net = match result {
            Ok(n) if !n.devices.is_empty() => n,
            other => {
                self.blind_polls += 1;
                if self.blind_polls == lost_after {
                    let at = SystemTime::now();
                    self.blind_since = Some(at);
                    let error = other.err().map(str::to_string).unwrap_or_else(|| "no adapter answered".to_string());
                    ev.push(Event::PollFailed { at, error });
                }
                return ev;
            }
        };
        let at = net.at;
        if self.blind_polls >= lost_after {
            let since = self.blind_since.unwrap_or(at);
            ev.push(Event::PollerOk { at, after_secs: at.duration_since(since).map(|d| d.as_secs()).unwrap_or(0) });
        }
        self.blind_polls = 0;
        self.blind_since = None;
        let first = self.prev.is_none();

        let answering: BTreeSet<Mac> = net.devices.iter().filter(|d| !d.seen_by_peers_only).map(|d| d.mac).collect();
        let present: BTreeSet<Mac> = net.devices.iter().map(|d| d.mac).collect();
        for mac in cfg.devices.keys().chain(cfg.spares.keys()).copied().filter(|m| cfg.is_owned(*m) && !cfg.is_spare(*m)) {
            if answering.contains(&mac) {
                if self.missing.remove(&mac).is_some_and(|n| n >= lost_after) && !first {
                    ev.push(Event::AdapterSeen { at, mac });
                }
            } else {
                let n = self.missing.entry(mac).or_insert(0);
                *n += 1;
                if *n == lost_after && !first {
                    ev.push(Event::AdapterLost { at, mac });
                }
            }
        }

        if !cfg.devices.is_empty() {
            for &mac in present.iter().filter(|m| !cfg.is_owned(**m)) {
                if self.unknown.insert(mac, 0).is_none() {
                    ev.push(Event::UnknownAdapterJoined { at, mac });
                }
            }
            let mut gone = Vec::new();
            for (mac, n) in self.unknown.iter_mut().filter(|(m, _)| !present.contains(m)) {
                *n += 1;
                if *n == lost_after {
                    gone.push(*mac);
                }
            }
            for mac in gone {
                self.unknown.remove(&mac);
                ev.push(Event::UnknownAdapterGone { at, mac });
            }
        }

        let mut seen_links: BTreeSet<(Mac, Mac)> = BTreeSet::new();
        for s in &net.snapshots {
            for l in &s.links {
                let key = (s.device, l.peer);
                seen_links.insert(key);
                let now = l.tx_mbps;
                let median = ring.median(key.0, key.1).filter(|_| ring.len() >= 20);
                let threshold = match median {
                    Some(m) => cfg.alerts.floor_mbps.max((u32::from(m) * (100 - u32::from(cfg.alerts.drop_pct)) / 100) as u16),
                    None => cfg.alerts.floor_mbps,
                };
                if now < threshold {
                    self.above.remove(&key);
                    let e = self.below.entry(key).or_insert((0, now, at));
                    e.0 += 1;
                    e.1 = e.1.min(now);
                    if e.0 == degraded_after && !self.degraded.contains(&key) && !first {
                        self.degraded.insert(key);
                        ev.push(Event::RateDegraded { at, from: key.0, to: key.1, was: median.unwrap_or(threshold), now });
                    }
                } else if self.degraded.contains(&key) {
                    let n = self.above.entry(key).or_insert(0);
                    *n += 1;
                    if *n == degraded_after {
                        let (_, lowest, since) = self.below.remove(&key).unwrap_or((0, now, at));
                        self.above.remove(&key);
                        self.degraded.remove(&key);
                        let after_secs = at.duration_since(since).map(|d| d.as_secs()).unwrap_or(0);
                        ev.push(Event::RateRecovered { at, from: key.0, to: key.1, now, lowest, after_secs });
                    }
                } else {
                    self.below.remove(&key);
                }
            }
        }
        let degraded = &self.degraded;
        self.below.retain(|k, _| seen_links.contains(k) || degraded.contains(k));
        self.above.retain(|k, _| seen_links.contains(k));

        if let Some(p) = &self.prev {
            if p.cco() != net.cco() {
                ev.push(Event::CcoChanged { at, was: p.cco(), now: net.cco() });
            }
            if let (Some(was), Some(now)) = (p.nid(), net.nid()) {
                if was != now {
                    ev.push(Event::NidChanged { at, was, now });
                }
            }
            for s in &net.snapshots {
                let Some(now_up) = s.uptime_secs else { continue };
                let Some(was_up) = p.snapshot(s.device).and_then(|ps| ps.uptime_secs) else { continue };
                if u64::from(now_up) + cfg.poll_seconds < u64::from(was_up) {
                    ev.push(Event::AdapterRebooted { at, mac: s.device, was_up_secs: was_up, now_up_secs: now_up });
                }
            }
        }
        if net.default_key() && !self.prev.as_ref().is_some_and(|p| p.default_key()) {
            ev.push(Event::DefaultKeyDetected { at, nid: net.nid().unwrap() });
        }
        let own = net.nid();
        for n in net.snapshots.iter().flat_map(|s| s.neighbour_nets.iter()).filter(|n| Some(n.nid) != own) {
            if self.heard.insert(n.nid) {
                ev.push(Event::NetworkHeard {
                    at,
                    nid: n.nid,
                    default_key: keys::is_default_nid(&n.nid),
                    own_default: net.default_key(),
                });
            }
        }
        self.prev = Some(net.clone());
        ev
    }
}
