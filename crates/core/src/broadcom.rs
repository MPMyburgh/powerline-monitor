use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::frame::{Mme, ET_MXT, OUI_BROADCOM};
use crate::model::{Device, JoinStatus, Link, LinkFlags, Mac, Nid, Role, Snapshot, Vendor};
use crate::sock::Sock;

const DISCOVER_MAGIC: [u8; 16] = [
    0xa3, 0x97, 0xa2, 0x55, 0x53, 0xbe, 0xf1, 0xfc, 0xf9, 0x79, 0x6b, 0x52, 0x14, 0x13, 0xe9, 0xe2,
];
const DISCOVER_TIMEOUT: Duration = Duration::from_millis(500);
const UNICAST_TIMEOUT: Duration = Duration::from_millis(300);

pub struct NetInfo {
    pub nid: Nid,
    pub snid: u8,
    pub tei: u8,
    pub role: Role,
    pub cco: Option<Mac>,
    pub bcco: Option<Mac>,
    pub kind: u8,
    pub coord_networks: u8,
    pub status: JoinStatus,
}

/// Seeded from the clock so concurrent pollers on this host use different sequence numbers.
fn next_seq() -> u8 {
    static SEQ: OnceLock<AtomicU8> = OnceLock::new();
    SEQ.get_or_init(|| {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        AtomicU8::new(nanos as u8)
    })
    .fetch_add(1, Ordering::Relaxed)
}

fn req(sock: &Sock, dst: Mac, mmv: u8, mmtype: u16, payload: Vec<u8>) -> Mme {
    Mme {
        src: sock.mac(),
        dst,
        ethertype: ET_MXT,
        mmv,
        mmtype,
        oui: Some(OUI_BROADCOM),
        seq: Some(next_seq()),
        fmi: None,
        payload,
    }
}

fn ask(sock: &Sock, dst: Mac, mmtype: u16, payload: Vec<u8>) -> Result<Vec<u8>> {
    let replies = sock.request(&req(sock, dst, 0x02, mmtype, payload), UNICAST_TIMEOUT)?;
    replies
        .into_iter()
        .next()
        .map(|m| m.payload)
        .ok_or_else(|| anyhow!("{dst}: no reply to 0x{mmtype:04x}"))
}

pub(crate) fn field<'a>(p: &'a [u8], range: std::ops::Range<usize>, what: &str) -> Result<&'a [u8]> {
    p.get(range).ok_or_else(|| anyhow!("short reply: missing {what}"))
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim_end_matches('\0').to_string()
}

fn nonzero(mac: Mac) -> Option<Mac> {
    (mac != Mac([0; 6])).then_some(mac)
}

fn parse_discover(m: &Mme, iface: &str) -> Result<Device> {
    let p = &m.payload;
    let len = *field(p, 1..2, "HFID length")?.first().unwrap() as usize;
    let hfid = text(field(p, 2..2 + len, "HFID")?);
    Ok(Device {
        mac: m.src,
        vendor: Vendor::Broadcom,
        hfid: Some(hfid),
        local: p[0] <= 1,
        iface: iface.to_string(),
        caps: None,
        ident: None,
        seen_by_peers_only: false,
    })
}

/// Broadcast discover, merged by source MAC. Returns once every expected adapter has answered;
/// one retry round goes out when some are still missing.
pub fn discover(sock: &Sock, expected: &BTreeSet<Mac>) -> Result<Vec<Device>> {
    let mut devices: Vec<Device> = Vec::new();
    for _ in 0..2 {
        let mut done =
            |replies: &[Mme]| !expected.is_empty() && expected.iter().all(|m| replies.iter().any(|r| r.src == *m));
        let req = req(sock, Mac::BROADCAST, 0x01, 0xA070, DISCOVER_MAGIC.to_vec());
        for m in sock.request_until(&req, DISCOVER_TIMEOUT, &mut done)? {
            if !devices.iter().any(|d| d.mac == m.src) {
                match parse_discover(&m, sock.iface()) {
                    Ok(d) => devices.push(d),
                    Err(e) => eprintln!("warning: {}: discover reply: {e:#}", m.src),
                }
            }
        }
        if expected.iter().all(|m| devices.iter().any(|d| d.mac == *m)) {
            break;
        }
    }
    Ok(devices)
}

/// Unicast discover to one adapter; Ok(None) when it stays silent.
pub fn discover_one(sock: &Sock, mac: Mac) -> Result<Option<Device>> {
    let replies = sock.request(&req(sock, mac, 0x01, 0xA070, DISCOVER_MAGIC.to_vec()), UNICAST_TIMEOUT)?;
    replies.first().map(|m| parse_discover(m, sock.iface())).transpose()
}

pub fn network_info(sock: &Sock, mac: Mac) -> Result<NetInfo> {
    let p = ask(sock, mac, 0xA028, vec![0x00, 0x00])?;
    let n = *field(&p, 0..1, "network count")?.first().unwrap() as usize;
    if n == 0 {
        bail!("{mac}: not joined to any network");
    }
    let e = field(&p, 1..20, "network entry")?;
    let bcco = p.get(1 + 19 * n..7 + 19 * n).map(Mac::from_slice).and_then(nonzero);
    Ok(NetInfo {
        nid: Nid(e[0..7].try_into().unwrap()),
        snid: e[7] & 0x0f,
        tei: e[8],
        role: match e[9] {
            0 => Role::UnassocSta,
            1 => Role::UnassocCco,
            2 => Role::Sta,
            3 => Role::Cco,
            4 => Role::BackupCco,
            _ => Role::Unknown,
        },
        cco: nonzero(Mac::from_slice(&e[10..16])),
        bcco,
        kind: e[16],
        coord_networks: e[17],
        status: JoinStatus::from_byte(e[18]),
    })
}

pub fn network_stats(sock: &Sock, mac: Mac, nid: Nid) -> Result<Vec<Link>> {
    let mut payload = vec![0x00];
    payload.extend_from_slice(&nid.0);
    let p = ask(sock, mac, 0xA02C, payload)?;
    let n = *field(&p, 0..1, "station count")?.first().unwrap() as usize;
    let mut links = Vec::with_capacity(n);
    for i in 0..n {
        let e = field(&p, 1 + 10 * i..11 + 10 * i, "station entry")?;
        let tx = u16::from_le_bytes([e[6], e[7]]);
        let rx = u16::from_le_bytes([e[8], e[9]]);
        links.push(Link {
            peer: Mac::from_slice(&e[0..6]),
            tx_mbps: tx & 0x07ff,
            rx_mbps: rx & 0x07ff,
            tx_flags: LinkFlags::from_word(tx),
            rx_flags: LinkFlags::from_word(rx),
        });
    }
    Ok(links)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StationInfo {
    pub firmware: String,
    pub firmware_name: String,
    pub built: String,
    pub chip_version: u32,
    pub hw_version: u32,
    pub chip_id: u32,
    pub uptime_secs: u32,
    pub hpav2: bool,
    pub rate_class: u8,
}

fn u32_at(p: &[u8], at: usize, what: &str) -> Result<u32> {
    let b = field(p, at..at + 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// 0xA04C: fixed ids, then a microcode table of 5 octets per entry, then uptime, boot string, firmware tag.
pub fn station_info(sock: &Sock, mac: Mac) -> Result<StationInfo> {
    let p = ask(sock, mac, 0xA04C, vec![])?;
    let nuc = *field(&p, 24..25, "ucode count")?.first().unwrap() as usize;
    let mut i = 25 + 5 * nuc;
    let uptime_secs = u32_at(&p, i, "uptime")?;
    i += 4;
    let blen = *field(&p, i..i + 1, "boot string length")?.first().unwrap() as usize;
    i += 1;
    let built = text(field(&p, i..i + blen, "boot string")?).trim().to_string();
    i += blen;
    let fw = field(&p, i..i + 4, "firmware version")?;
    let firmware_name = match fw[3] {
        0 => "CONCORDE",
        5 => "GEMINI",
        6 => "APOLLO",
        7 => "HYDRA",
        _ => "UNKNOWN",
    }
    .to_string();
    let firmware = format!("{}.{}.{} (svn {})", fw[2], fw[1], fw[0], u32_at(&p, 8, "firmware revision")?);
    i += 12;
    let tail = field(&p, i..i + 2, "hpav and rate class")?;
    Ok(StationInfo {
        firmware,
        firmware_name,
        built,
        chip_version: u32_at(&p, 0, "chip version")?,
        hw_version: u32_at(&p, 4, "hardware version")?,
        chip_id: u32_at(&p, 12, "chip id")?,
        uptime_secs,
        hpav2: tail[0] == 1,
        rate_class: tail[1],
    })
}

pub fn snapshot(sock: &Sock, mac: Mac) -> Result<Snapshot> {
    let info = network_info(sock, mac)?;
    let links = network_stats(sock, mac, info.nid)?;
    let si = station_info(sock, mac).map_err(|e| eprintln!("warning: {mac}: station info: {e:#}")).ok();
    let at = SystemTime::now();
    Ok(Snapshot {
        device: mac,
        nid: info.nid,
        snid: info.snid,
        tei: info.tei,
        role: info.role,
        cco: info.cco,
        bcco: info.bcco,
        status: info.status,
        kind: info.kind,
        joined: info.status == JoinStatus::Joined,
        links,
        heard: Vec::new(),
        neighbour_nets: Vec::new(),
        bridged: Vec::new(),
        firmware: si.as_ref().map(|s| s.firmware.clone()),
        uptime_secs: si.as_ref().map(|s| s.uptime_secs),
        powered_on_since: si.as_ref().and_then(|s| at.checked_sub(Duration::from_secs(s.uptime_secs.into()))),
        rate_class: si.as_ref().map(|s| s.rate_class),
        at,
    })
}
