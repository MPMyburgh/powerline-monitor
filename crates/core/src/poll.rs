use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::broadcom;
use crate::config::Config;
use crate::frame::OUI_BROADCOM;
use crate::model::{Device, Mac, NetState, Vendor};
use crate::sock::{Sock, SockError};
use crate::standard::{self, StaCap, StaIdentify};

const OUI_QUALCOMM: [u8; 3] = [0x00, 0xb0, 0x52];
const CAPS_REFRESH_POLLS: u64 = 20;

pub struct Poller {
    expected: BTreeSet<Mac>,
    caps: BTreeMap<Mac, (Option<StaCap>, Option<StaIdentify>)>,
    polls: u64,
}

pub fn poll_once(cfg: &Config) -> Result<NetState> {
    Poller::new(cfg).poll(cfg)
}

fn oui_vendor(oui: [u8; 3]) -> Vendor {
    match oui {
        OUI_BROADCOM => Vendor::Broadcom,
        OUI_QUALCOMM => Vendor::Qualcomm,
        _ => Vendor::Unknown,
    }
}

impl Poller {
    pub fn new(cfg: &Config) -> Poller {
        Poller { expected: cfg.devices.keys().chain(cfg.spares.keys()).copied().collect(), caps: BTreeMap::new(), polls: 0 }
    }

    /// Discover on every interface, walk the peers the answers list, then snapshot each Broadcom adapter.
    /// An adapter seen on several interfaces keeps the one where it is local. An interface that cannot be
    /// opened is skipped with a warning unless every interface fails.
    pub fn poll(&mut self, cfg: &Config) -> Result<NetState> {
        self.polls += 1;
        if self.polls.is_multiple_of(CAPS_REFRESH_POLLS) {
            self.caps.clear();
        }
        let mut state = NetState { devices: Vec::new(), snapshots: Vec::new(), at: SystemTime::now() };
        let ifaces = cfg.effective_interfaces();
        if ifaces.is_empty() {
            return Err(SockError("no ethernet interface is up; plug in the adapter or set interfaces".to_string()).into());
        }
        let mut errors = Vec::new();
        for iface in &ifaces {
            let (sock, found) = match Sock::open(iface).and_then(|s| broadcom::discover(&s, &self.expected).map(|d| (s, d))) {
                Ok(x) => x,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            for dev in found {
                self.adopt(&sock, cfg, dev, &mut state);
            }
            let known: BTreeSet<Mac> = state.devices.iter().map(|d| d.mac).collect();
            let mut walk: BTreeSet<Mac> = BTreeSet::new();
            for s in state.snapshots.iter().filter(|s| known.contains(&s.device)) {
                walk.extend(s.links.iter().map(|l| l.peer));
                walk.extend(s.heard.iter().filter(|h| h.same_network).map(|h| h.mac));
            }
            for mac in walk.difference(&known) {
                match broadcom::discover_one(&sock, *mac) {
                    Ok(Some(dev)) => self.adopt(&sock, cfg, dev, &mut state),
                    Ok(None) => state.devices.push(Device {
                        mac: *mac,
                        vendor: Vendor::Unknown,
                        hfid: None,
                        local: false,
                        iface: iface.clone(),
                        caps: None,
                        ident: None,
                        seen_by_peers_only: true,
                    }),
                    Err(e) => eprintln!("warning: {iface}: {e:#}"),
                }
            }
        }
        if errors.len() == ifaces.len() {
            return Err(errors.remove(0));
        }
        for e in errors {
            eprintln!("warning: {e:#}");
        }
        self.expected = state
            .devices
            .iter()
            .filter(|d| !d.seen_by_peers_only)
            .map(|d| d.mac)
            .chain(cfg.devices.keys().copied())
            .chain(cfg.spares.keys().copied())
            .collect();
        Ok(state)
    }

    /// Capability lookup (cached), OUI-gated vendor, then the vendor snapshot and the standard station lists.
    fn adopt(&mut self, sock: &Sock, cfg: &Config, mut dev: Device, state: &mut NetState) {
        if let Some(seen) = state.devices.iter_mut().find(|d| d.mac == dev.mac) {
            if dev.local && !seen.local {
                dev.caps = seen.caps.take();
                dev.ident = seen.ident.take();
                *seen = dev;
            }
            return;
        }
        let (caps, ident) = self
            .caps
            .entry(dev.mac)
            .or_insert_with(|| (standard::sta_cap(sock, dev.mac).ok(), standard::sta_identify(sock, dev.mac).ok()))
            .clone();
        dev.caps = caps;
        dev.ident = ident;
        if let Some(c) = &dev.caps {
            dev.vendor = oui_vendor(c.oui);
        }
        if let Some(v) = cfg.devices.get(&dev.mac).and_then(|d| d.vendor) {
            if let Some(c) = dev.caps.as_ref().filter(|c| oui_vendor(c.oui) != v) {
                eprintln!(
                    "warning: {}: config says {v}, adapter reports OUI {:02x}:{:02x}:{:02x}",
                    dev.mac, c.oui[0], c.oui[1], c.oui[2]
                );
            }
            dev.vendor = v;
        }
        if dev.vendor == Vendor::Broadcom {
            match broadcom::snapshot(sock, dev.mac).or_else(|_| broadcom::snapshot(sock, dev.mac)) {
                Ok(mut s) => {
                    match standard::discover_list(sock, dev.mac) {
                        Ok(dl) => {
                            s.heard = dl.stations;
                            s.neighbour_nets = dl.networks;
                        }
                        Err(e) => eprintln!("warning: {}: {e:#}", dev.mac),
                    }
                    match standard::bridge_info(sock, dev.mac) {
                        Ok(b) => s.bridged = b,
                        Err(e) => eprintln!("warning: {}: {e:#}", dev.mac),
                    }
                    state.snapshots.push(s);
                }
                Err(e) => eprintln!("warning: {}: {e:#}", sock.iface()),
            }
        }
        state.devices.push(dev);
    }
}
