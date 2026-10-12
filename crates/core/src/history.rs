use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::model::{Mac, NetState};
use crate::text;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LinkSample {
    pub from: Mac,
    pub to: Mac,
    pub tx: u16,
    pub rx: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Poll {
    #[serde(with = "crate::model::rfc3339")]
    pub at: SystemTime,
    pub links: Vec<LinkSample>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LinkStats {
    pub now: u16,
    pub min: u16,
    pub avg: u16,
    pub samples: usize,
    pub median: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ring {
    capacity: usize,
    polls: VecDeque<Poll>,
}

impl Ring {
    pub fn new(capacity: usize) -> Ring {
        Ring { capacity: capacity.max(1), polls: VecDeque::new() }
    }

    pub fn from_config(cfg: &Config) -> Ring {
        Ring::new(capacity_for(cfg))
    }

    /// Changes the capacity in place, dropping the oldest polls only when over the new size.
    pub fn resize(&mut self, cfg: &Config) {
        self.capacity = capacity_for(cfg).max(1);
        while self.polls.len() > self.capacity {
            self.polls.pop_front();
        }
    }

    pub fn push(&mut self, state: &NetState) {
        let links = state
            .snapshots
            .iter()
            .flat_map(|s| {
                s.links.iter().map(move |l| LinkSample { from: s.device, to: l.peer, tx: l.tx_mbps, rx: l.rx_mbps })
            })
            .collect();
        self.polls.push_back(Poll { at: state.at, links });
        while self.polls.len() > self.capacity {
            self.polls.pop_front();
        }
    }

    pub fn series(&self, from: Mac, to: Mac) -> Vec<(SystemTime, u16, u16)> {
        self.polls
            .iter()
            .filter_map(|p| {
                let l = p.links.iter().find(|l| l.from == from && l.to == to)?;
                Some((p.at, l.tx, l.rx))
            })
            .collect()
    }

    /// Over the transmitter's TX figure for the directed link.
    pub fn stats(&self, from: Mac, to: Mac) -> Option<LinkStats> {
        let s = self.series(from, to);
        let now = s.last()?.1;
        let min = s.iter().map(|x| x.1).min()?;
        let avg = (s.iter().map(|x| x.1 as u64).sum::<u64>() / s.len() as u64) as u16;
        let median = median_of(s.iter().map(|x| x.1).collect())?;
        Some(LinkStats { now, min, avg, samples: s.len(), median })
    }

    pub fn len(&self) -> usize {
        self.polls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.polls.is_empty()
    }

    pub fn oldest(&self) -> Option<SystemTime> {
        self.polls.front().map(|p| p.at)
    }

    pub fn median(&self, from: Mac, to: Mac) -> Option<u16> {
        self.percentile(from, to, 50.0)
    }

    /// Nearest-rank percentile, `p` in 0..=100, over TX of the whole ring.
    pub fn percentile(&self, from: Mac, to: Mac, p: f64) -> Option<u16> {
        let mut v: Vec<u16> = self.series(from, to).iter().map(|x| x.1).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        Some(v[rank(v.len(), p)])
    }

    pub fn window_min(&self, from: Mac, to: Mac, last_n: usize) -> Option<u16> {
        self.series(from, to).iter().rev().take(last_n).map(|x| x.1).min()
    }

    /// One bucket per hour ending now, oldest first; None where no sample fell in the hour.
    pub fn hourly_medians(&self, from: Mac, to: Mac, hours: usize) -> Vec<Option<u16>> {
        let now = SystemTime::now();
        let s = self.series(from, to);
        (0..hours)
            .map(|i| {
                let end = now.checked_sub(Duration::from_secs((hours - 1 - i) as u64 * 3600)).unwrap_or(UNIX_EPOCH);
                let start = end.checked_sub(Duration::from_secs(3600)).unwrap_or(UNIX_EPOCH);
                median_of(s.iter().filter(|x| x.0 > start && x.0 <= end).map(|x| x.1).collect())
            })
            .collect()
    }

    pub fn load(path: &Path) -> Result<Ring> {
        let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut ring: Ring = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        ring.capacity = ring.capacity.max(1);
        Ok(ring)
    }

    /// Writes `path.tmp` then renames it over `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, serde_json::to_vec(self)?).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
    }

    /// Union by poll time (this ring wins on equal times), sorted, oldest dropped over capacity.
    pub fn merge(&mut self, other: &Ring) {
        let mut all: BTreeMap<SystemTime, Poll> = self.polls.drain(..).map(|p| (p.at, p)).collect();
        for p in &other.polls {
            all.entry(p.at).or_insert_with(|| p.clone());
        }
        self.polls = all.into_values().collect();
        while self.polls.len() > self.capacity {
            self.polls.pop_front();
        }
    }

    pub fn to_csv(&self, cfg: &Config, filter: Option<(Mac, Mac)>) -> String {
        let mut out = String::from("time,from,from_name,to,to_name,tx_mbps,rx_mbps\n");
        for p in &self.polls {
            let time = text::stamp_full(p.at);
            for l in p.links.iter().filter(|l| filter.is_none_or(|(f, t)| l.from == f && l.to == t)) {
                out.push_str(&format!(
                    "{time},{},{},{},{},{},{}\n",
                    l.from,
                    csv_field(&cfg.name(l.from)),
                    l.to,
                    csv_field(&cfg.name(l.to)),
                    l.tx,
                    l.rx
                ));
            }
        }
        out
    }
}

pub fn history_path() -> PathBuf {
    Config::state_dir().join("history.json")
}

pub fn tracker_path() -> PathBuf {
    Config::state_dir().join("tracker.json")
}

fn capacity_for(cfg: &Config) -> usize {
    (cfg.history_hours * 3600 / cfg.poll_seconds.max(1)) as usize
}

fn rank(n: usize, p: f64) -> usize {
    ((p / 100.0 * n as f64).ceil() as usize).clamp(1, n) - 1
}

fn median_of(mut v: Vec<u16>) -> Option<u16> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[rank(v.len(), 50.0)])
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
