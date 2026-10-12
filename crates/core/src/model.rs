use anyhow::{anyhow, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use crate::keys;
use crate::standard::{HeardNetwork, HeardStation, StaCap, StaIdentify};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub const BROADCAST: Mac = Mac([0xff; 6]);

    pub fn from_slice(b: &[u8]) -> Mac {
        let mut m = [0u8; 6];
        m.copy_from_slice(&b[..6]);
        Mac(m)
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}

impl FromStr for Mac {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Mac, anyhow::Error> {
        let parts: Vec<&str> = s.split([':', '-']).collect();
        if parts.len() != 6 {
            anyhow::bail!("bad MAC address {s:?}");
        }
        let mut m = [0u8; 6];
        for (i, p) in parts.iter().enumerate() {
            m[i] = u8::from_str_radix(p, 16).map_err(|_| anyhow::anyhow!("bad MAC address {s:?}"))?;
        }
        Ok(Mac(m))
    }
}

impl Serialize for Mac {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Mac {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Mac, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Network ID in wire order; Display is colon-hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Nid(pub [u8; 7]);

impl Nid {
    /// Bits 5-4 of the last octet: 00 Simple Connect, else High Security.
    pub fn security_level(&self) -> &'static str {
        if self.0[6] & 0x30 == 0 {
            "Simple Connect"
        } else {
            "High Security"
        }
    }
}

impl fmt::Display for Nid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Nid {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Nid, anyhow::Error> {
        let hex: String = s.chars().filter(|c| !matches!(c, ':' | '-')).collect();
        if hex.len() != 14 || !hex.is_ascii() {
            bail!("bad NID {s:?}");
        }
        let mut n = [0u8; 7];
        for (i, b) in n.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| anyhow!("bad NID {s:?}"))?;
        }
        Ok(Nid(n))
    }
}

impl Serialize for Nid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Nid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Nid, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Bytes([u8; 7]),
        }
        match Repr::deserialize(d)? {
            Repr::Text(s) => s.parse().map_err(serde::de::Error::custom),
            Repr::Bytes(b) => Ok(Nid(b)),
        }
    }
}

/// RFC 3339 with the local UTC offset, e.g. 2026-10-07T20:45:28+02:00, for `#[serde(with = "crate::model::rfc3339")]`.
pub mod rfc3339 {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn format(t: SystemTime) -> String {
        let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let tt = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&tt, &mut tm) };
        let sign = if tm.tm_gmtoff < 0 { '-' } else { '+' };
        let off = tm.tm_gmtoff.abs();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            off / 3600,
            off % 3600 / 60
        )
    }

    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// Accepts "YYYY-MM-DDTHH:MM:SS[.frac](Z|+HH:MM|-HH:MM)".
    pub fn parse(s: &str) -> Option<SystemTime> {
        let b = s.as_bytes();
        let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
        if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
        let mut i = 19;
        let mut nanos = 0u32;
        if b.get(i) == Some(&b'.') {
            let start = i + 1;
            i = start;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            nanos = format!("{:0<9}", &s[start..i])[..9].parse().ok()?;
        }
        let off = match *b.get(i)? {
            b'Z' | b'z' => 0,
            sign @ (b'+' | b'-') => {
                if b.get(i + 3) != Some(&b':') {
                    return None;
                }
                let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
                if sign == b'-' {
                    -o
                } else {
                    o
                }
            }
            _ => return None,
        };
        let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - off;
        Some(UNIX_EPOCH + Duration::new(u64::try_from(secs).ok()?, nanos))
    }

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(*t))
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Text(String),
        Secs(u64),
        Legacy { secs_since_epoch: u64, nanos_since_epoch: u32 },
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        match Repr::deserialize(d)? {
            Repr::Text(s) => parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad timestamp {s:?}"))),
            Repr::Secs(n) => UNIX_EPOCH.checked_add(Duration::from_secs(n)).ok_or_else(|| serde::de::Error::custom("timestamp out of range")),
            Repr::Legacy { secs_since_epoch, nanos_since_epoch } => UNIX_EPOCH
                .checked_add(Duration::new(secs_since_epoch, nanos_since_epoch))
                .ok_or_else(|| serde::de::Error::custom("timestamp out of range")),
        }
    }

    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer};
        use std::time::SystemTime;

        pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
            match t {
                Some(t) => s.serialize_some(&super::format(*t)),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
            #[derive(Deserialize)]
            struct Wrap(#[serde(with = "super")] SystemTime);
            Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Broadcom,
    Qualcomm,
    Unknown,
}

impl fmt::Display for Vendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Vendor::Broadcom => "broadcom",
            Vendor::Qualcomm => "qualcomm",
            Vendor::Unknown => "unknown",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Cco,
    BackupCco,
    Sta,
    UnassocCco,
    UnassocSta,
    Unknown,
}

impl Role {
    pub fn human(self) -> &'static str {
        match self {
            Role::Cco => "Coordinator",
            Role::BackupCco => "Backup coordinator",
            Role::Sta => "Member",
            Role::UnassocCco => "Joining (coordinator)",
            Role::UnassocSta => "Joining",
            Role::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Cco => "CCo",
            Role::BackupCco => "bCCo",
            Role::Sta => "STA",
            Role::UnassocCco => "uCCo",
            Role::UnassocSta => "uSTA",
            Role::Unknown => "-",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinStatus {
    Joined,
    HasKeyNotJoined,
    NoKey,
    Unknown(u8),
}

impl JoinStatus {
    pub fn from_byte(b: u8) -> JoinStatus {
        match b {
            0 => JoinStatus::Joined,
            1 => JoinStatus::HasKeyNotJoined,
            2 => JoinStatus::NoKey,
            n => JoinStatus::Unknown(n),
        }
    }
}

impl fmt::Display for JoinStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinStatus::Joined => f.write_str("joined"),
            JoinStatus::HasKeyNotJoined => f.write_str("has the key, still joining"),
            JoinStatus::NoKey => f.write_str("rejected or no key"),
            JoinStatus::Unknown(n) => write!(f, "status {n}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spec {
    Av11,
    Av20,
    Ieee1901,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mimo {
    Siso1,
    Siso2,
    Mimo,
    SisoOnly,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LinkFlags {
    pub spec: Spec,
    pub mimo: Mimo,
    pub beamforming: bool,
}

impl LinkFlags {
    /// Bits 15-14 spec, 13-12 MIMO, 11 spot beamforming of a 0xA02D rate word.
    pub fn from_word(w: u16) -> LinkFlags {
        LinkFlags {
            spec: match w >> 14 {
                0 => Spec::Av11,
                1 => Spec::Av20,
                _ => Spec::Ieee1901,
            },
            mimo: match (w >> 12) & 3 {
                0 => Mimo::Siso1,
                1 => Mimo::Siso2,
                2 => Mimo::Mimo,
                _ => Mimo::SisoOnly,
            },
            beamforming: w & 0x0800 != 0,
        }
    }

    pub fn label(&self) -> String {
        let spec = match self.spec {
            Spec::Av11 => "AV1.1",
            Spec::Av20 => "AV2.0",
            Spec::Ieee1901 => "IEEE 1901",
        };
        let mimo = match self.mimo {
            Mimo::Siso1 => "SISO",
            Mimo::Siso2 => "SISO 2",
            Mimo::Mimo => "MIMO",
            Mimo::SisoOnly => "SISO only",
        };
        let bf = if self.beamforming { " beamforming" } else { "" };
        format!("{spec} {mimo}{bf}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub mac: Mac,
    pub vendor: Vendor,
    pub hfid: Option<String>,
    pub local: bool,
    pub iface: String,
    pub caps: Option<StaCap>,
    pub ident: Option<StaIdentify>,
    pub seen_by_peers_only: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Link {
    pub peer: Mac,
    pub tx_mbps: u16,
    pub rx_mbps: u16,
    pub tx_flags: LinkFlags,
    pub rx_flags: LinkFlags,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub device: Mac,
    pub nid: Nid,
    pub snid: u8,
    pub tei: u8,
    pub role: Role,
    pub cco: Option<Mac>,
    pub bcco: Option<Mac>,
    pub status: JoinStatus,
    pub kind: u8,
    pub joined: bool,
    pub links: Vec<Link>,
    pub heard: Vec<HeardStation>,
    pub neighbour_nets: Vec<HeardNetwork>,
    pub bridged: Vec<Mac>,
    pub firmware: Option<String>,
    pub uptime_secs: Option<u32>,
    #[serde(with = "rfc3339::option")]
    pub powered_on_since: Option<SystemTime>,
    pub rate_class: Option<u8>,
    #[serde(with = "rfc3339")]
    pub at: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetState {
    pub devices: Vec<Device>,
    pub snapshots: Vec<Snapshot>,
    #[serde(with = "rfc3339")]
    pub at: SystemTime,
}

/// '49 d 05 h', '3 h 12 min', '7 min', '45 s'.
pub fn human_uptime(secs: u32) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d} d {h:02} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{secs} s")
    }
}

fn majority<T: PartialEq + Copy>(items: impl Iterator<Item = T>) -> Option<T> {
    let mut counts: Vec<(T, usize)> = Vec::new();
    for it in items {
        match counts.iter_mut().find(|(v, _)| *v == it) {
            Some(c) => c.1 += 1,
            None => counts.push((it, 1)),
        }
    }
    counts.iter().max_by_key(|(_, n)| *n).map(|(v, _)| *v)
}

impl NetState {
    pub fn snapshot(&self, mac: Mac) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.device == mac)
    }

    pub fn rate(&self, from: Mac, to: Mac) -> Option<(u16, u16)> {
        let l = self.snapshot(from)?.links.iter().find(|l| l.peer == to)?;
        Some((l.tx_mbps, l.rx_mbps))
    }

    pub fn nid(&self) -> Option<Nid> {
        majority(self.snapshots.iter().map(|s| s.nid))
    }

    pub fn cco(&self) -> Option<Mac> {
        majority(self.snapshots.iter().filter_map(|s| s.cco))
    }

    pub fn bcco(&self) -> Option<Mac> {
        majority(self.snapshots.iter().filter_map(|s| s.bcco))
    }

    pub fn default_key(&self) -> bool {
        self.nid().is_some_and(|n| keys::is_default_nid(&n))
    }

    /// Local adapters first, then ascending MAC.
    pub fn macs(&self) -> Vec<Mac> {
        let mut v: Vec<(bool, Mac)> = self.devices.iter().map(|d| (!d.local, d.mac)).collect();
        v.sort();
        v.dedup();
        v.into_iter().map(|(_, m)| m).collect()
    }
}
