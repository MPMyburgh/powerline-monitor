use anyhow::{bail, Context, Result};
use serde::de::{Deserializer, IntoDeserializer};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, TableLike, Value};

use crate::events::Severity;
use crate::model::{Mac, Vendor};

const EXAMPLE: &str = include_str!("../../../packaging/powerline-monitor.toml.example");

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceCfg {
    pub alias: Option<String>,
    pub vendor: Option<Vendor>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Alerts {
    pub lost_after_polls: u32,
    pub degraded_after_polls: u32,
    pub floor_mbps: u16,
    pub drop_pct: u8,
}

impl Default for Alerts {
    fn default() -> Alerts {
        Alerts { lost_after_polls: 3, degraded_after_polls: 10, floor_mbps: 50, drop_pct: 40 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    #[serde(alias = "on_event", deserialize_with = "string_or_vec")]
    pub exec: Vec<String>,
    pub min_severity: Severity,
}

impl Default for Hooks {
    fn default() -> Hooks {
        Hooks { exec: Vec::new(), min_severity: Severity::Warning }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

fn string_or_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match StringOrVec::deserialize(d)? {
        StringOrVec::One(s) => vec![s],
        StringOrVec::Many(v) => v,
    })
}

/// `Option<Severity>` as the severity name, with None spelled "off".
mod severity_or_off {
    use super::*;

    pub fn serialize<S: serde::Serializer>(v: &Option<Severity>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(sev) => sev.serialize(s),
            None => s.serialize_str("off"),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Severity>, D::Error> {
        let s = String::deserialize(d)?;
        if s == "off" {
            return Ok(None);
        }
        Severity::deserialize(s.as_str().into_deserializer()).map(Some)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub interfaces: Vec<String>,
    pub poll_seconds: u64,
    pub history_hours: u64,
    pub listen: String,
    pub daemon_url: Option<String>,
    #[serde(with = "severity_or_off")]
    pub notify_min_severity: Option<Severity>,
    pub alerts: Alerts,
    pub hooks: Hooks,
    pub devices: BTreeMap<Mac, DeviceCfg>,
    pub spares: BTreeMap<Mac, String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            interfaces: Vec::new(),
            poll_seconds: 30,
            history_hours: 24,
            listen: "127.0.0.1:9702".to_string(),
            daemon_url: None,
            notify_min_severity: Some(Severity::Warning),
            alerts: Alerts::default(),
            hooks: Hooks::default(),
            devices: BTreeMap::new(),
            spares: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct ResolveError(pub String);

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResolveError {}

impl Config {
    /// Lookup: `path`, else $XDG_CONFIG_HOME/powerline-monitor/config.toml, else /etc/powerline-monitor.toml.
    /// A missing default file gives defaults; the returned path is where save() will write.
    pub fn load(path: Option<&Path>) -> Result<(Config, PathBuf)> {
        let explicit = path.is_some();
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => {
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
        };
        let cfg = if path.exists() {
            let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            Config::parse(&text).with_context(|| format!("parsing {}", path.display()))?
        } else if explicit {
            bail!("config {} not found", path.display());
        } else {
            Config::default()
        };
        Ok((cfg, path))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(text)?;
        cfg.check()?;
        Ok(cfg)
    }

    pub fn check(&self) -> Result<()> {
        range("poll_seconds", self.poll_seconds, 5, 600)?;
        range("history_hours", self.history_hours, 1, 720)?;
        range("alerts.lost_after_polls", self.alerts.lost_after_polls, 1, 20)?;
        range("alerts.degraded_after_polls", self.alerts.degraded_after_polls, 1, 100)?;
        range("alerts.floor_mbps", self.alerts.floor_mbps, 1, 2000)?;
        range("alerts.drop_pct", self.alerts.drop_pct, 1, 90)?;
        if !self.listen.is_empty() && self.listen.parse::<SocketAddr>().is_err() {
            bail!("listen must be an address like 127.0.0.1:9702, not {:?}", self.listen);
        }
        let aliases = self.devices.iter().filter_map(|(m, d)| d.alias.as_deref().map(|a| (*m, a, "alias")));
        let spares = self.spares.iter().map(|(m, n)| (*m, n.as_str(), "spare name"));
        let mut used: BTreeMap<String, Mac> = BTreeMap::new();
        for (mac, name, what) in aliases.chain(spares) {
            if name.trim().is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
                bail!("{what} for {mac} must be 1..=32 printable characters");
            }
            if mac_like(name) {
                bail!("{what} {name:?} for {mac} looks like a MAC address");
            }
            if default_name_suffix(name).is_some() {
                bail!("{what} {name:?} for {mac} is a default name; choose another");
            }
            if let Some(other) = used.insert(name.to_lowercase(), mac) {
                bail!("{what} {name:?} is used by both {other} and {mac}");
            }
        }
        if let Some(mac) = self.devices.keys().find(|m| self.spares.contains_key(m)) {
            bail!("{mac} is in both [devices] and [spares]");
        }
        Ok(())
    }

    /// Alias, else spare name, else "Adapter 1B2C" from the last two MAC octets.
    pub fn name(&self, mac: Mac) -> String {
        self.devices
            .get(&mac)
            .and_then(|d| d.alias.clone())
            .or_else(|| self.spares.get(&mac).cloned())
            .unwrap_or_else(|| format!("Adapter {:02X}{:02X}", mac.0[4], mac.0[5]))
    }

    pub fn is_owned(&self, mac: Mac) -> bool {
        self.devices.contains_key(&mac) || self.spares.contains_key(&mac)
    }

    pub fn is_spare(&self, mac: Mac) -> bool {
        self.spares.contains_key(&mac)
    }

    /// Accepts a name (case-insensitive), "Adapter XXXX", a full MAC, or a MAC suffix of
    /// four or more hex digits that is unique over the configured and `seen` adapters.
    pub fn resolve_adapter(&self, arg: &str, seen: &[Mac]) -> Result<Mac> {
        let mut known: Vec<Mac> = seen.to_vec();
        known.extend(self.devices.keys().chain(self.spares.keys()).copied());
        let mut uniq = BTreeSet::new();
        known.retain(|m| uniq.insert(*m));
        if let Some(m) = known.iter().find(|m| self.name(**m).eq_ignore_ascii_case(arg)) {
            return Ok(*m);
        }
        if let Ok(m) = arg.parse::<Mac>() {
            return Ok(m);
        }
        let hex: String = default_name_suffix(arg)
            .unwrap_or(arg)
            .chars()
            .filter(|c| !matches!(c, ':' | '-'))
            .collect::<String>()
            .to_lowercase();
        let matches: Vec<Mac> = if hex.len() >= 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            known.iter().copied().filter(|m| m.to_string().replace(':', "").ends_with(&hex)).collect()
        } else {
            Vec::new()
        };
        match matches.as_slice() {
            [m] => Ok(*m),
            [] => {
                let names: Vec<String> = known.iter().map(|m| self.name(*m)).collect();
                let names = if names.is_empty() { "none".to_string() } else { names.join(", ") };
                Err(ResolveError(format!("no adapter matches {arg:?}; known: {names}")).into())
            }
            _ => {
                let macs: Vec<String> = matches.iter().map(ToString::to_string).collect();
                Err(ResolveError(format!("ambiguous adapter {arg:?}: {}", macs.join(", "))).into())
            }
        }
    }

    /// Edits the existing file in place (comments and unrelated keys survive); starts from the
    /// packaged example when the file is absent. Keys are written only when they differ.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => EXAMPLE.to_string(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut doc: DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
        let base = Config::default();
        let root = doc.as_table_mut();
        put(root, "interfaces", strings(&self.interfaces), self.interfaces == base.interfaces);
        put(root, "poll_seconds", int(self.poll_seconds), self.poll_seconds == base.poll_seconds);
        put(root, "history_hours", int(self.history_hours), self.history_hours == base.history_hours);
        put(root, "listen", Value::from(self.listen.as_str()), self.listen == base.listen);
        opt_str(root, "daemon_url", self.daemon_url.as_deref());
        let notify = self.notify_min_severity.map_or_else(|| "off".to_string(), sev_name);
        put(root, "notify_min_severity", Value::from(notify.as_str()), self.notify_min_severity == base.notify_min_severity);

        let (a, b) = (self.alerts, Alerts::default());
        if let Some(t) = section(&mut doc, "alerts", a != b)? {
            put(t, "lost_after_polls", int(a.lost_after_polls), a.lost_after_polls == b.lost_after_polls);
            put(t, "degraded_after_polls", int(a.degraded_after_polls), a.degraded_after_polls == b.degraded_after_polls);
            put(t, "floor_mbps", int(a.floor_mbps), a.floor_mbps == b.floor_mbps);
            put(t, "drop_pct", int(a.drop_pct), a.drop_pct == b.drop_pct);
        }
        let (h, hb) = (&self.hooks, Hooks::default());
        if let Some(t) = section(&mut doc, "hooks", *h != hb)? {
            let key = if t.contains_key("on_event") { "on_event" } else { "exec" };
            put(t, key, strings(&h.exec), h.exec == hb.exec);
            put(t, "min_severity", Value::from(sev_name(h.min_severity).as_str()), h.min_severity == hb.min_severity);
        }
        if let Some(t) = section(&mut doc, "spares", !self.spares.is_empty())? {
            let keys = mac_keys(t);
            for (k, mac) in &keys {
                if !mac.is_some_and(|m| self.spares.contains_key(&m)) {
                    t.remove(k);
                }
            }
            for (mac, name) in &self.spares {
                opt_str(t, &key_for(&keys, *mac), Some(name));
            }
            if t.is_empty() {
                doc.remove("spares");
            }
        }
        if let Some(devs) = section(&mut doc, "devices", !self.devices.is_empty())? {
            let keys = mac_keys(devs);
            for (k, mac) in &keys {
                if !mac.is_some_and(|m| self.devices.contains_key(&m)) {
                    devs.remove(k);
                }
            }
            for (mac, d) in &self.devices {
                let key = key_for(&keys, *mac);
                let t = devs.entry(&key).or_insert(Item::Table(Table::new()));
                let t = t.as_table_like_mut().with_context(|| format!("devices.{key:?} is not a table"))?;
                opt_str(t, "alias", d.alias.as_deref());
                opt_str(t, "vendor", d.vendor.map(|v| v.to_string()).as_deref());
            }
            if devs.is_empty() {
                doc.remove("devices");
            }
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))
    }

    /// Configured interfaces, else every interface that is up and has /sys/class/net/<if>/type == 1.
    pub fn effective_interfaces(&self) -> Vec<String> {
        if !self.interfaces.is_empty() {
            return self.interfaces.clone();
        }
        let mut v: Vec<String> = fs::read_dir("/sys/class/net")
            .map(|d| {
                d.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().into_string().ok()?;
                        let p = e.path();
                        let up = fs::read_to_string(p.join("operstate")).ok()?.trim() == "up";
                        let eth = fs::read_to_string(p.join("type")).ok()?.trim() == "1";
                        (up && eth).then_some(name)
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// $STATE_DIRECTORY, else $XDG_STATE_HOME/powerline-monitor, else ~/.local/state/powerline-monitor.
    pub fn state_dir() -> PathBuf {
        if let Some(d) = env::var_os("STATE_DIRECTORY") {
            return PathBuf::from(d);
        }
        if let Some(d) = env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(d).join("powerline-monitor");
        }
        env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/state/powerline-monitor"))
            .unwrap_or_else(|| PathBuf::from("/var/lib/powerline-monitor"))
    }

    pub fn ensure_state_dir() -> Result<PathBuf> {
        let dir = Config::state_dir();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }
}

fn range<T: PartialOrd + fmt::Display>(key: &str, v: T, lo: T, hi: T) -> Result<()> {
    if v < lo || v > hi {
        bail!("{key} must be {lo}..={hi}");
    }
    Ok(())
}

fn mac_like(s: &str) -> bool {
    let hex: String = s.chars().filter(|c| !matches!(c, ':' | '-')).collect();
    hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// The "XXXX" of a default "Adapter XXXX" name, case-insensitive.
fn default_name_suffix(s: &str) -> Option<&str> {
    let rest = s.get(..8)?.eq_ignore_ascii_case("adapter ").then(|| &s[8..])?;
    (rest.len() == 4 && rest.chars().all(|c| c.is_ascii_hexdigit())).then_some(rest)
}

fn sev_name(s: Severity) -> String {
    toml::Value::try_from(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn int<T: Into<u64>>(v: T) -> Value {
    Value::from(v.into() as i64)
}

fn strings(v: &[String]) -> Value {
    let mut a = Array::new();
    for s in v {
        a.push(s.as_str());
    }
    Value::Array(a)
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same(p, q)),
        _ => false,
    }
}

/// Replaces an existing value when it differs (keeping its comments); inserts a missing
/// one only when it is not the default.
fn put(t: &mut dyn TableLike, key: &str, mut new: Value, is_default: bool) {
    match t.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            if !same(old, &new) {
                *new.decor_mut() = old.decor().clone();
                *old = new;
            }
        }
        None if is_default => {}
        None => {
            t.insert(key, Item::Value(new));
        }
    }
}

fn opt_str(t: &mut dyn TableLike, key: &str, v: Option<&str>) {
    match v {
        Some(s) => put(t, key, Value::from(s), false),
        None => {
            t.remove(key);
        }
    }
}

/// The `[key]` table, created only when `needed`; None when absent and not needed.
fn section<'a>(doc: &'a mut DocumentMut, key: &str, needed: bool) -> Result<Option<&'a mut dyn TableLike>> {
    if !needed && doc.get(key).is_none() {
        return Ok(None);
    }
    let item = doc.entry(key).or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(key == "devices");
        Item::Table(t)
    });
    Ok(Some(item.as_table_like_mut().with_context(|| format!("[{key}] is not a table"))?))
}

fn mac_keys(t: &dyn TableLike) -> Vec<(String, Option<Mac>)> {
    t.iter().map(|(k, _)| (k.to_string(), k.parse().ok())).collect()
}

/// The key already used for `mac` in the file (any separator or case), else its canonical form.
fn key_for(keys: &[(String, Option<Mac>)], mac: Mac) -> String {
    keys.iter().find(|(_, m)| *m == Some(mac)).map_or_else(|| mac.to_string(), |(k, _)| k.clone())
}
