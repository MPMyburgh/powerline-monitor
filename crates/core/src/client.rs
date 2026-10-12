use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer};

use crate::events::Event;
use crate::history::Ring;
use crate::model::NetState;

pub struct Client {
    base: String,
    timeout: Duration,
}

/// The daemon could not be reached; main.rs maps this to exit 4.
#[derive(Debug)]
pub struct Unreachable {
    pub url: String,
    pub error: std::io::Error,
}

impl fmt::Display for Unreachable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot reach powerline-monitor daemon at {}: {}", self.url, self.error)
    }
}

impl std::error::Error for Unreachable {}

#[derive(Clone, Debug, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub interfaces: Vec<String>,
    pub poll_seconds: u64,
    #[serde(default, deserialize_with = "opt_time")]
    pub last_poll_at: Option<SystemTime>,
    pub last_poll_ok: bool,
    pub polls: u64,
}

fn opt_time<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
    #[derive(Deserialize)]
    struct Stamp(#[serde(with = "crate::model::rfc3339")] SystemTime);
    Ok(Option::<Stamp>::deserialize(d)?.map(|s| s.0))
}

impl Client {
    /// Accepts "http://host:port" or "host:port".
    pub fn new(url: &str) -> Client {
        let base = url.trim().trim_end_matches('/');
        let base = base.strip_prefix("http://").unwrap_or(base).to_string();
        Client { base, timeout: Duration::from_secs(2) }
    }

    pub fn with_timeout(self, d: Duration) -> Client {
        Client { timeout: d, ..self }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.base)
    }

    fn get(&self, path: &str) -> Result<(u16, String)> {
        let io = |error| Unreachable { url: self.url(), error };
        let addrs = self.base.to_socket_addrs().map_err(io)?;
        let mut stream = None;
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no address");
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, self.timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let mut stream = stream.ok_or_else(|| io(last))?;
        stream.set_read_timeout(Some(self.timeout)).map_err(io)?;
        stream.set_write_timeout(Some(self.timeout)).map_err(io)?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n", self.base);
        stream.write_all(req.as_bytes()).map_err(io)?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(io)?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").with_context(|| format!("malformed reply from {}", self.url()))?;
        let head = String::from_utf8_lossy(&raw[..split]);
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .with_context(|| format!("malformed status line from {}", self.url()))?;
        let chunked = head.lines().any(|l| {
            let l = l.to_ascii_lowercase();
            l.starts_with("transfer-encoding:") && l.contains("chunked")
        });
        let body = if chunked { dechunk(&raw[split + 4..]) } else { raw[split + 4..].to_vec() };
        Ok((status, String::from_utf8_lossy(&body).to_string()))
    }

    fn get_ok(&self, path: &str) -> Result<String> {
        let (status, body) = self.get(path)?;
        if status != 200 {
            bail!("daemon at {} returned {status} for {path}", self.url());
        }
        Ok(body)
    }

    /// None when the daemon has not polled yet (503, or a null body from older daemons).
    pub fn state(&self) -> Result<Option<NetState>> {
        let (status, body) = self.get("/api/state")?;
        match status {
            503 => Ok(None),
            200 => serde_json::from_str(&body).context("decoding /api/state"),
            _ => bail!("daemon at {} returned {status} for /api/state", self.url()),
        }
    }

    pub fn history(&self) -> Result<Ring> {
        serde_json::from_str(&self.get_ok("/api/history")?).context("decoding /api/history")
    }

    pub fn events(&self, since: Option<SystemTime>) -> Result<Vec<Event>> {
        let path = match since.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
            Some(d) => format!("/api/events?since={}", d.as_secs()),
            None => "/api/events".to_string(),
        };
        serde_json::from_str(&self.get_ok(&path)?).context("decoding /api/events")
    }

    pub fn status(&self) -> Result<DaemonStatus> {
        serde_json::from_str(&self.get_ok("/api/status")?).context("decoding /api/status")
    }
}

/// Joins the chunks of a chunked transfer-encoded body, stopping at the zero-size chunk.
fn dechunk(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let Some(eol) = b[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = String::from_utf8_lossy(&b[i..i + eol]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        i += eol + 2;
        if size == 0 || i + size > b.len() {
            break;
        }
        out.extend_from_slice(&b[i..i + size]);
        i += size + 2;
    }
    out
}
