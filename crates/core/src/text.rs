use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};

use crate::model::rfc3339;

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub fn local_tm(secs: i64) -> libc::tm {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

fn epoch(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Local calendar day number of `secs`.
fn day_number(secs: i64) -> i64 {
    (secs + local_tm(secs).tm_gmtoff).div_euclid(86400)
}

/// Some("Today"/"Yesterday"), else None for the local day of `t`.
fn relative_day(t: SystemTime) -> Option<&'static str> {
    match day_number(epoch(SystemTime::now())) - day_number(epoch(t)) {
        0 => Some("Today"),
        1 => Some("Yesterday"),
        _ => None,
    }
}

fn date(tm: &libc::tm) -> String {
    format!("{} {:02} {}", DAYS[tm.tm_wday as usize % 7], tm.tm_mday, MONTHS[tm.tm_mon as usize % 12])
}

fn stamp_with(t: SystemTime, seconds: bool) -> String {
    let tm = local_tm(epoch(t));
    let clock = if seconds {
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    } else {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    };
    match relative_day(t) {
        Some("Today") => clock,
        Some(day) => format!("{day} {clock}"),
        None => format!("{} {clock}", date(&tm)),
    }
}

/// "HH:MM" today, "Yesterday HH:MM", else "Tue 07 Oct HH:MM".
pub fn stamp(t: SystemTime) -> String {
    stamp_with(t, false)
}

pub fn stamp_seconds(t: SystemTime) -> String {
    stamp_with(t, true)
}

/// RFC 3339 with the local offset, e.g. "2026-10-07T20:45:28+02:00".
pub fn stamp_full(t: SystemTime) -> String {
    rfc3339::format(t)
}

pub fn hm(secs: f64) -> String {
    let tm = local_tm(secs as i64);
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

pub fn day_label(t: SystemTime) -> String {
    relative_day(t).map_or_else(|| date(&local_tm(epoch(t))), str::to_string)
}

pub fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).unwrap_or_default().as_secs();
    match s {
        0..=1 => "just now".to_string(),
        2..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h {} min ago", s / 3600, s % 3600 / 60),
        _ => format!("{} d ago", s / 86400),
    }
}

/// "45 s", "12 min", "1 h 15 min", "49 d 05 h".
pub fn duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86399 => format!("{} h {:02} min", secs / 3600, secs % 3600 / 60),
        _ => format!("{} d {:02} h", secs / 86400, secs % 86400 / 3600),
    }
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Accepts "30m" / "6h" / "2d" (ago), unix seconds, or RFC 3339 with offset.
pub fn parse_since(s: &str) -> Result<SystemTime> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix(['m', 'h', 'd']).and_then(|n| n.parse::<u64>().ok()) {
        let unit = match s.as_bytes()[s.len() - 1] {
            b'm' => 60,
            b'h' => 3600,
            _ => 86400,
        };
        let secs = n.checked_mul(unit).map(Duration::from_secs).unwrap_or(Duration::MAX);
        return Ok(SystemTime::now().checked_sub(secs).unwrap_or(UNIX_EPOCH));
    }
    if let Ok(n) = s.parse::<u64>() {
        return UNIX_EPOCH
            .checked_add(Duration::from_secs(n))
            .ok_or_else(|| anyhow::anyhow!("cannot parse time {s:?}: out of range"));
    }
    match rfc3339::parse(s) {
        Some(t) => Ok(t),
        None => bail!("cannot parse time {s:?}: use 30m, 6h, 2d, unix seconds or RFC 3339"),
    }
}
