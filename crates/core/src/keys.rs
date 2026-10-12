use sha2::{Digest, Sha256};

use crate::model::Nid;

const NMK_SALT: [u8; 8] = [0x08, 0x85, 0x6d, 0xaf, 0x7c, 0xf5, 0x81, 0x86];

pub const DEFAULT_NID: Nid = Nid([0xb0, 0xf2, 0xe6, 0x95, 0x66, 0x6b, 0x03]);

fn pbkdf1(data: &[u8], rounds: usize) -> [u8; 32] {
    let mut d = Sha256::digest(data);
    for _ in 1..rounds {
        d = Sha256::digest(d);
    }
    d.into()
}

pub fn nmk_from_passphrase(passphrase: &str) -> [u8; 16] {
    let d = pbkdf1(&[passphrase.as_bytes(), &NMK_SALT].concat(), 1000);
    d[..16].try_into().unwrap()
}

/// Security level Simple Connect (00) goes into bits 5-4 of the last octet.
pub fn nid_from_nmk(nmk: &[u8; 16]) -> Nid {
    let d = pbkdf1(nmk, 5);
    let mut nid: [u8; 7] = d[..7].try_into().unwrap();
    nid[6] >>= 4;
    Nid(nid)
}

/// Matches the factory NID at both security levels (octet 6 bits 5-4: 00 Simple Connect, 01 High Security).
pub fn is_default_nid(nid: &Nid) -> bool {
    nid.0[..6] == DEFAULT_NID.0[..6] && matches!(nid.0[6], 0x03 | 0x13)
}
