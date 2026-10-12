use anyhow::Result;
use std::ffi::CString;
use std::fmt;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use crate::frame::{Mme, ET_HPAV, ET_MXT};
use crate::model::Mac;

#[derive(Debug)]
pub struct SockError(pub String);

impl fmt::Display for SockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SockError {}

pub struct Sock {
    fd: OwnedFd,
    mac: Mac,
    iface: String,
}

/// Classic BPF: accept a frame only when its ethertype is 0x88E1 or 0x8912.
fn attach_filter(fd: i32) -> io::Result<()> {
    let mut prog = [
        libc::sock_filter { code: 0x28, jt: 0, jf: 0, k: 12 },
        libc::sock_filter { code: 0x15, jt: 1, jf: 0, k: ET_HPAV as u32 },
        libc::sock_filter { code: 0x15, jt: 0, jf: 1, k: ET_MXT as u32 },
        libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x40000 },
        libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0 },
    ];
    let fprog = libc::sock_fprog { len: prog.len() as u16, filter: prog.as_mut_ptr() };
    let r = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            &fprog as *const _ as *const libc::c_void,
            mem::size_of::<libc::sock_fprog>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl Sock {
    pub fn open(iface: &str) -> Result<Sock> {
        let name = CString::new(iface)?;
        let ifindex = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if ifindex == 0 {
            return Err(SockError(format!("interface {iface} not found")).into());
        }
        let proto = (libc::ETH_P_ALL as u16).to_be() as libc::c_int;
        let raw = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW | libc::SOCK_CLOEXEC, proto) };
        if raw < 0 {
            let e = io::Error::last_os_error();
            let msg = match e.raw_os_error() {
                Some(libc::EPERM) | Some(libc::EACCES) => format!(
                    "{iface}: opening a raw socket needs CAP_NET_RAW (run as root, or: sudo setcap cap_net_raw+ep <binary>)"
                ),
                _ => format!("{iface}: socket: {e}"),
            };
            return Err(SockError(msg).into());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        attach_filter(raw).map_err(|e| SockError(format!("{iface}: attach filter: {e}")))?;
        let mut addr: libc::sockaddr_ll = unsafe { mem::zeroed() };
        addr.sll_family = libc::AF_PACKET as u16;
        addr.sll_protocol = proto as u16;
        addr.sll_ifindex = ifindex as i32;
        let r = unsafe {
            libc::bind(
                raw,
                &addr as *const _ as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if r < 0 {
            return Err(SockError(format!("{iface}: bind: {}", io::Error::last_os_error())).into());
        }
        let mac = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))
            .map_err(|e| SockError(format!("{iface}: read MAC: {e}")))?
            .trim()
            .parse()
            .map_err(|_| SockError(format!("{iface}: no MAC address")))?;
        Ok(Sock { fd, mac, iface: iface.to_string() })
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    pub fn iface(&self) -> &str {
        &self.iface
    }

    pub fn send(&self, mme: &Mme) -> Result<()> {
        let f = mme.encode()?;
        let n = unsafe { libc::send(self.fd.as_raw_fd(), f.as_ptr() as *const libc::c_void, f.len(), 0) };
        if n < 0 {
            return Err(SockError(format!("{}: send: {}", self.iface, io::Error::last_os_error())).into());
        }
        Ok(())
    }

    /// Replies are matched on ethertype, OUI, CNF MMTYPE, sequence number and source MAC (unless broadcast).
    /// Unicast returns on the first match; a CM_MME_ERROR.IND naming the request fails at once.
    pub fn request(&self, req: &Mme, timeout: Duration) -> Result<Vec<Mme>> {
        self.request_until(req, timeout, &mut |_| false)
    }

    /// Like request, but also returns as soon as `done` says the replies collected so far are enough.
    pub fn request_until(&self, req: &Mme, timeout: Duration, done: &mut dyn FnMut(&[Mme]) -> bool) -> Result<Vec<Mme>> {
        self.send(req)?;
        let broadcast = req.dst == Mac::BROADCAST;
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut pfd = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let n = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(SockError(format!("{}: poll: {e}", self.iface)).into());
            }
            if n == 0 {
                break;
            }
            let len = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
            if len < 0 {
                continue;
            }
            let Some(m) = Mme::decode(&buf[..len as usize]) else { continue };
            if m.src == req.dst && m.mmtype == 0x6046 && m.payload.len() >= 4 && m.payload[2..4] == req.mmtype.to_le_bytes() {
                return Err(SockError(format!("{}: 0x{:04x} not supported by this adapter", req.dst, req.mmtype)).into());
            }
            if m.ethertype != req.ethertype || m.oui != req.oui || m.mmtype != (req.mmtype | 1) {
                continue;
            }
            if req.seq.is_some() && m.seq != req.seq {
                continue;
            }
            if !broadcast && m.src != req.dst {
                continue;
            }
            out.push(m);
            if !broadcast || done(&out) {
                break;
            }
        }
        Ok(out)
    }
}
