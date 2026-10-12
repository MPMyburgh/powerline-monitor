use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::broadcom::field;
use crate::frame::{Mme, ET_HPAV};
use crate::model::{Mac, Nid};
use crate::sock::Sock;

const UNICAST_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaCap {
    pub av_version: u8,
    pub oui: [u8; 3],
    pub cco_capable: bool,
    pub proxy_capable: bool,
    pub backup_cco_capable: bool,
    pub bidir_bursting: bool,
    pub impl_version: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StaIdentify {
    pub power_save_capable: bool,
    pub hpav: u8,
    pub mcs: u8,
    pub min_carrier: u16,
    pub max_carrier: u16,
}

impl StaIdentify {
    /// Carrier numbers times the 24.414 kHz spacing, in MHz.
    pub fn band_mhz(&self) -> (f32, f32) {
        (self.min_carrier as f32 * 0.024414, self.max_carrier as f32 * 0.024414)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeardStation {
    pub mac: Mac,
    pub tei: u8,
    pub same_network: bool,
    pub snid: u8,
    pub is_cco: bool,
    pub is_pco: bool,
    pub is_backup_cco: bool,
    pub signal_level: u8,
    pub avg_ble: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeardNetwork {
    pub nid: Nid,
    pub snid: u8,
    pub hybrid_mode: u8,
    pub coordinating: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiscoverList {
    pub stations: Vec<HeardStation>,
    pub networks: Vec<HeardNetwork>,
}

fn ask(sock: &Sock, dst: Mac, mmtype: u16) -> Result<Mme> {
    let req = Mme { src: sock.mac(), dst, ethertype: ET_HPAV, mmv: 0x01, mmtype, oui: None, seq: None, fmi: None, payload: vec![] };
    sock.request(&req, UNICAST_TIMEOUT)?.into_iter().next().ok_or_else(|| anyhow!("{dst}: no reply to 0x{mmtype:04x}"))
}

pub fn sta_cap(sock: &Sock, mac: Mac) -> Result<StaCap> {
    let p = ask(sock, mac, 0x6034)?.payload;
    let e = field(&p, 0..25, "CM_STA_CAP body")?;
    Ok(StaCap {
        av_version: e[0],
        oui: [e[7], e[8], e[9]],
        cco_capable: e[12] != 0,
        proxy_capable: e[13] != 0,
        backup_cco_capable: e[14] != 0,
        bidir_bursting: e[22] != 0,
        impl_version: u16::from_le_bytes([e[23], e[24]]),
    })
}

pub fn sta_identify(sock: &Sock, mac: Mac) -> Result<StaIdentify> {
    let p = ask(sock, mac, 0x6060)?.payload;
    let e = field(&p, 0..16, "CM_STA_IDENTIFY body")?;
    Ok(StaIdentify {
        power_save_capable: e[1] != 0,
        hpav: e[4],
        mcs: e[6],
        min_carrier: u16::from_le_bytes([e[12], e[13]]),
        max_carrier: u16::from_le_bytes([e[14], e[15]]),
    })
}

pub fn bridge_info(sock: &Sock, mac: Mac) -> Result<Vec<Mac>> {
    let p = ask(sock, mac, 0x6020)?.payload;
    if *field(&p, 0..1, "bridging flag")?.first().unwrap() == 0 {
        return Ok(Vec::new());
    }
    let n = *field(&p, 2..3, "bridged count")?.first().unwrap() as usize;
    let macs = field(&p, 3..3 + 6 * n, "bridged MACs")?;
    Ok(macs.chunks_exact(6).map(Mac::from_slice).collect())
}

pub fn discover_list(sock: &Sock, mac: Mac) -> Result<DiscoverList> {
    let m = ask(sock, mac, 0x0014)?;
    if m.fmi.is_some_and(|(nf, _)| nf >> 4 != 0) {
        bail!("fragmented CC_DISCOVER_LIST not supported");
    }
    let p = &m.payload;
    let n = *field(p, 0..1, "station count")?.first().unwrap() as usize;
    let mut out = DiscoverList::default();
    for i in 0..n {
        let e = field(p, 1 + 12 * i..13 + 12 * i, "station entry")?;
        out.stations.push(HeardStation {
            mac: Mac::from_slice(&e[0..6]),
            tei: e[6],
            same_network: e[7] != 0,
            snid: e[8] & 0x0f,
            is_cco: e[9] & 0x20 != 0,
            is_pco: e[9] & 0x40 != 0,
            is_backup_cco: e[9] & 0x80 != 0,
            signal_level: e[10],
            avg_ble: e[11],
        });
    }
    let base = 1 + 12 * n;
    let nn = *field(p, base..base + 1, "network count")?.first().unwrap() as usize;
    for i in 0..nn {
        let e = field(p, base + 1 + 13 * i..base + 14 + 13 * i, "network entry")?;
        out.networks.push(HeardNetwork {
            nid: Nid(e[0..7].try_into().unwrap()),
            snid: e[7] & 0x0f,
            hybrid_mode: e[8],
            coordinating: e[10],
        });
    }
    Ok(out)
}

pub fn signal_level_text(level: u8) -> String {
    match level {
        0 => "n/a".to_string(),
        1 => "-10..0 dB".to_string(),
        15.. => "<= -75 dB".to_string(),
        l => format!("{}..{} dB", -(5 * l as i32 + 5), -(5 * l as i32)),
    }
}
