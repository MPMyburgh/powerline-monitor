use anyhow::{bail, Result};

use crate::model::Mac;

pub const ET_HPAV: u16 = 0x88E1;
pub const ET_MXT: u16 = 0x8912;
pub const OUI_BROADCOM: [u8; 3] = [0x00, 0x1f, 0x84];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sendable {
    pub ethertype: u16,
    pub oui: Option<[u8; 3]>,
    pub mmtype: u16,
}

const fn bcm(mmtype: u16) -> Sendable {
    Sendable { ethertype: ET_MXT, oui: Some(OUI_BROADCOM), mmtype }
}

const fn std(mmtype: u16) -> Sendable {
    Sendable { ethertype: ET_HPAV, oui: None, mmtype }
}

pub const SENDABLE: &[Sendable] =
    &[bcm(0xA070), bcm(0xA028), bcm(0xA02C), bcm(0xA018), bcm(0xA04C), std(0x0014), std(0x6020), std(0x6034), std(0x6060)];

#[derive(Clone, Debug)]
pub struct Mme {
    pub src: Mac,
    pub dst: Mac,
    pub ethertype: u16,
    pub mmv: u8,
    pub mmtype: u16,
    pub oui: Option<[u8; 3]>,
    pub seq: Option<u8>,
    /// (NF_MI/FN_MI, FMSN) of a 0x88E1 frame with MMV >= 1; set by decode only.
    pub fmi: Option<(u8, u8)>,
    pub payload: Vec<u8>,
}

fn is_vendor(mmtype: u16) -> bool {
    (0xA000..0xC000).contains(&mmtype)
}

impl Mme {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let key = Sendable { ethertype: self.ethertype, oui: self.oui, mmtype: self.mmtype };
        if !SENDABLE.contains(&key) {
            let oui = match self.oui {
                Some(o) => format!("{:02x}:{:02x}:{:02x}", o[0], o[1], o[2]),
                None => "-".to_string(),
            };
            bail!(
                "refusing to send 0x{:04x} on 0x{:04x}/{oui}: not on the SENDABLE allowlist",
                self.mmtype,
                self.ethertype
            );
        }
        let mut f = Vec::with_capacity(60 + self.payload.len());
        f.extend_from_slice(&self.dst.0);
        f.extend_from_slice(&self.src.0);
        f.extend_from_slice(&self.ethertype.to_be_bytes());
        f.push(self.mmv);
        f.extend_from_slice(&self.mmtype.to_le_bytes());
        if self.ethertype == ET_MXT {
            f.extend_from_slice(&[0, 0]);
            f.extend_from_slice(&self.oui.unwrap_or(OUI_BROADCOM));
            f.push(self.seq.unwrap_or(0));
        } else {
            if self.mmv >= 1 {
                f.extend_from_slice(&[0, 0]);
            }
            if let Some(oui) = self.oui {
                f.extend_from_slice(&oui);
            }
        }
        f.extend_from_slice(&self.payload);
        f.resize(f.len().max(60), 0);
        Ok(f)
    }

    pub fn decode(b: &[u8]) -> Option<Mme> {
        if b.len() < 17 {
            return None;
        }
        let ethertype = u16::from_be_bytes([b[12], b[13]]);
        if ethertype != ET_HPAV && ethertype != ET_MXT {
            return None;
        }
        let mmv = b[14];
        let mmtype = u16::from_le_bytes([b[15], b[16]]);
        let (oui, seq, start) = if ethertype == ET_MXT && is_vendor(mmtype) {
            if b.len() < 23 {
                return None;
            }
            (Some([b[19], b[20], b[21]]), Some(b[22]), 23)
        } else if mmv == 0 {
            if is_vendor(mmtype) && b.len() >= 20 {
                (Some([b[17], b[18], b[19]]), None, 20)
            } else {
                (None, None, 17)
            }
        } else if is_vendor(mmtype) && b.len() >= 22 {
            (Some([b[19], b[20], b[21]]), None, 22)
        } else {
            (None, None, 19)
        };
        if b.len() < start {
            return None;
        }
        let fmi = (ethertype == ET_HPAV && mmv >= 1).then(|| (b[17], b[18]));
        Some(Mme {
            src: Mac::from_slice(&b[6..12]),
            dst: Mac::from_slice(&b[0..6]),
            ethertype,
            mmv,
            mmtype,
            oui,
            seq,
            fmi,
            payload: b[start..].to_vec(),
        })
    }
}
