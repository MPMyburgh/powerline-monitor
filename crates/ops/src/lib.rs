use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use powerline_core::broadcom;
use powerline_core::config::Config;
use powerline_core::frame::{Mme, ET_MXT, OUI_BROADCOM};
use powerline_core::keys;
use powerline_core::model::{JoinStatus, Mac, NetState, Nid, Role, Vendor};
use powerline_core::poll::poll_once;
use powerline_core::sock::Sock;
use serde::{Serialize, Serializer};
use serde_json::json;

const SET_NMK_TIMEOUT: Duration = Duration::from_secs(2);
const REACH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(clap::Args, Debug, Clone, Default)]
pub struct RekeyArgs {
    /// Read the passphrase from one line of stdin instead of prompting twice without echo
    #[arg(long)]
    pub passphrase_stdin: bool,
    /// Accept 8 to 63 characters instead of 24 to 64
    #[arg(long)]
    pub allow_short: bool,
    /// Write only to these adapters (repeatable); also the only way to write an adapter that is not in the config
    #[arg(long, value_name = "MAC")]
    pub only: Vec<Mac>,
    /// Poll and print the plan, send nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Skip the typed REKEY confirmation
    #[arg(long)]
    pub yes: bool,
    /// Seconds to wait for every adapter to rejoin on the new NID
    #[arg(long, default_value_t = 60, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub timeout_seconds: u64,
    /// Set by main after parsing; named in the refusal when adapters are not in the config
    #[arg(skip)]
    pub config_path: PathBuf,
}

#[derive(clap::Args, Debug, Clone, Default)]
pub struct CheckArgs {
    /// Print the per-adapter result as JSON
    #[arg(long)]
    pub json: bool,
}

/// A preflight or safety refusal: nothing was written. main maps it to exit 5.
#[derive(Debug)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

fn refused<T>(msg: String) -> Result<T> {
    Err(Refused(msg).into())
}

#[derive(Clone)]
pub struct Nmk([u8; 16]);

impl fmt::Debug for Nmk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Nmk(<redacted>)")
    }
}

impl fmt::Display for Nmk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for Nmk {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("<redacted>")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub mac: Mac,
    pub name: String,
    pub role: Role,
    pub local: bool,
    pub iface: String,
    pub current_nid: Option<Nid>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub nid: Nid,
    nmk: Nmk,
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub mac: Mac,
    pub accepted: bool,
    pub converged: bool,
    pub status: Option<JoinStatus>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub nid: Nid,
    pub outcomes: Vec<Outcome>,
    pub elapsed: Duration,
    pub interrupted: bool,
}

fn table(rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..rows[0].len()).map(|c| rows.iter().map(|r| r[c].len()).max().unwrap()).collect();
    rows.iter()
        .map(|r| {
            let line: Vec<String> = r.iter().enumerate().map(|(c, v)| format!("{:<w$}", v, w = widths[c])).collect();
            format!("{}\n", line.join("  ").trim_end())
        })
        .collect()
}

pub fn check_default_key(cfg: &Config, args: &CheckArgs) -> Result<()> {
    let net = poll_once(cfg)?;
    if net.snapshots.is_empty() {
        eprintln!("no adapter answered on {}", cfg.effective_interfaces().join(", "));
        exit(3);
    }
    let rows: Vec<(Mac, Option<Nid>)> = net.macs().into_iter().map(|m| (m, net.snapshot(m).map(|s| s.nid))).collect();
    let answered = rows.iter().filter(|(_, n)| n.is_some()).count();
    let default = rows.iter().filter(|(_, n)| n.is_some_and(|n| keys::is_default_nid(&n))).count();
    if args.json {
        let adapters: Vec<_> = rows
            .iter()
            .map(|(m, n)| {
                json!({ "mac": m, "name": cfg.name(*m), "nid": n, "default_nid": n.map(|n| keys::is_default_nid(&n)) })
            })
            .collect();
        let out = json!({ "adapters": adapters, "answered": answered, "default_key": default, "pass": default == 0 });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Tests the factory passphrase \"HomePlugAV\": each adapter's NID is compared with the NID derived from it. No key is read from the adapters.\n");
        let mut t = vec![["NAME", "MAC", "NID", "NID IS DEFAULT"].map(String::from).to_vec()];
        for (m, n) in &rows {
            t.push(vec![
                cfg.name(*m),
                m.to_string(),
                n.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string()),
                match n {
                    Some(n) if keys::is_default_nid(n) => "yes",
                    Some(_) => "no",
                    None => "no answer",
                }
                .to_string(),
            ]);
        }
        println!("{}", table(&t));
        if default == 0 {
            println!("PASS  0 of {answered} adapters use the factory key.");
        } else {
            println!("FAIL  {default} of {answered} adapters use the factory key. Any HomePlug AV adapter on this\n      wiring, yours or a neighbour's, joins your LAN without pairing.\n      Fix: powerline-monitor rekey");
        }
    }
    if default > 0 {
        exit(2);
    }
    Ok(())
}

fn validate_passphrase(p: &str, min: usize, max: usize) -> Result<()> {
    if !p.chars().all(|c| (' '..='~').contains(&c)) {
        return refused("passphrase must be printable ASCII".to_string());
    }
    if p.len() < min || p.len() > max {
        return refused(format!("passphrase must be {min}-{max} characters (got {})", p.len()));
    }
    Ok(())
}

fn read_passphrase(from_stdin: bool) -> Result<String> {
    if from_stdin {
        let mut s = String::new();
        io::stdin().lock().read_line(&mut s)?;
        return Ok(s.trim_end_matches(['\r', '\n']).to_string());
    }
    let a = rpassword::prompt_password("New network passphrase: ")?;
    let b = rpassword::prompt_password("Again: ")?;
    if a != b {
        return refused("passphrases do not match".to_string());
    }
    Ok(a)
}

/// Reads the confirmation from the controlling terminal, not stdin, so a pipe cannot supply it.
fn confirm(n: usize) -> Result<bool> {
    let Ok(tty) = File::open("/dev/tty") else {
        return refused("no terminal to confirm on; run from a terminal or pass --yes".to_string());
    };
    print!("Type REKEY to write the new key to {n} adapter(s), anything else aborts: ");
    io::stdout().flush()?;
    let mut s = String::new();
    BufReader::new(tty).read_line(&mut s)?;
    Ok(s.trim() == "REKEY")
}

/// Broadcom 0xA018: NMK, NID type 0x00 (derive from the NMK), security level 0x00 (Simple Connect).
fn set_nmk_request(src: Mac, dst: Mac, nmk: &Nmk) -> Mme {
    let mut payload = nmk.0.to_vec();
    payload.extend_from_slice(&[0x00, 0x00]);
    let seq = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u8).unwrap_or(2);
    Mme { src, dst, ethertype: ET_MXT, mmv: 0x02, mmtype: 0xA018, oui: Some(OUI_BROADCOM), seq: Some(seq), fmi: None, payload }
}

/// Targets are the Broadcom adapters seen (filtered by `only`); each must be in cfg.devices or named in `only`.
/// Order: remote STA/bCCo by MAC, then the remote CCo, the local adapter last.
pub(crate) fn plan(cfg: &Config, net: &NetState, passphrase: &str, only: &[Mac], config_path: &Path) -> Result<Plan> {
    validate_passphrase(passphrase, 8, 64)?;
    let nmk = keys::nmk_from_passphrase(passphrase);
    let nid = keys::nid_from_nmk(&nmk);
    if keys::is_default_nid(&nid) {
        return refused("refusing the factory passphrase \"HomePlugAV\"".to_string());
    }
    let mut steps: Vec<Step> = net
        .devices
        .iter()
        .filter(|d| d.vendor == Vendor::Broadcom && (only.is_empty() || only.contains(&d.mac)))
        .map(|d| Step {
            mac: d.mac,
            name: cfg.name(d.mac),
            role: net.snapshot(d.mac).map(|s| s.role).unwrap_or(Role::Unknown),
            local: d.local,
            iface: d.iface.clone(),
            current_nid: net.snapshot(d.mac).map(|s| s.nid),
        })
        .collect();
    for m in only {
        if !steps.iter().any(|s| s.mac == *m) {
            return refused(format!("--only {m}: not a Broadcom adapter seen on this poll"));
        }
    }
    if steps.is_empty() {
        return refused(format!("no Broadcom adapters to rekey on {}", cfg.effective_interfaces().join(", ")));
    }
    let strangers: Vec<String> =
        steps.iter().filter(|s| !cfg.is_owned(s.mac) && !only.contains(&s.mac)).map(|s| s.mac.to_string()).collect();
    if !strangers.is_empty() {
        return refused(format!(
            "not named in the config: {}. Add them with: powerline-monitor discover --toml >> {}  (or name them in the Settings tab), then re-run; or name every target with --only",
            strangers.join(", "),
            config_path.display()
        ));
    }
    if steps.iter().all(|s| s.current_nid == Some(nid)) {
        return refused("every target already uses the NID derived from this passphrase".to_string());
    }
    steps.sort_by_key(|s| (s.local, s.role == Role::Cco, s.mac));
    Ok(Plan { steps, nid, nmk: Nmk(nmk) })
}

fn plan_text(plan: &Plan) -> String {
    let mut rows = vec![["#", "NAME", "MAC", "ROLE", "VIA", "CURRENT NID", "NEW NID"].map(String::from).to_vec()];
    for (i, s) in plan.steps.iter().enumerate() {
        rows.push(vec![
            (i + 1).to_string(),
            s.name.clone(),
            s.mac.to_string(),
            s.role.human().to_lowercase(),
            if s.local { "local" } else { "plc" }.to_string(),
            s.current_nid.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string()),
            plan.nid.to_string(),
        ]);
    }
    format!(
        "Rekey plan: {} adapter(s) move to NID {} (SL-SC). Remote adapters go first over the powerline (the CCo last among them), the local one last; adapters on different keys cannot see each other until all have moved. Nothing has been written.\n{}",
        plan.steps.len(),
        plan.nid,
        table(&rows)
    )
}

/// Writes the key to each step in order, stopping at the first unconfirmed write or on Ctrl-C, then polls
/// until every accepted target reports JOINED on the new NID with the others in its station list, or `timeout` passes.
pub(crate) fn execute(cfg: &Config, plan: &Plan, timeout: Duration) -> Result<Report> {
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))?;
    }
    let mut outcomes: Vec<Outcome> = Vec::new();
    let sock = Sock::open(&plan.steps[0].iface)?;
    for step in &plan.steps {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if !step.local {
            let until = Instant::now() + REACH_TIMEOUT;
            while broadcom::network_info(&sock, step.mac).is_err() && Instant::now() < until && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let mut result = Err("no reply".to_string());
        for _ in 0..2 {
            let req = set_nmk_request(sock.mac(), step.mac, &plan.nmk);
            match sock.request(&req, SET_NMK_TIMEOUT) {
                Ok(replies) => match replies.first().and_then(|m| m.payload.first().copied()) {
                    Some(0) => result = Ok(()),
                    Some(code) => result = Err(format!("rejected, status 0x{code:02x}")),
                    None => continue,
                },
                Err(e) => result = Err(e.to_string()),
            }
            break;
        }
        let accepted = result.is_ok();
        outcomes.push(Outcome { mac: step.mac, accepted, converged: false, status: None, error: result.err() });
        if !accepted {
            break;
        }
    }
    let accepted: Vec<Mac> = outcomes.iter().filter(|o| o.accepted).map(|o| o.mac).collect();
    let deadline = Instant::now() + timeout;
    while !accepted.is_empty() {
        match poll_once(cfg) {
            Ok(net) => {
                for o in outcomes.iter_mut() {
                    let snap = net.snapshot(o.mac).filter(|s| s.nid == plan.nid);
                    o.status = snap.map(|s| s.status);
                    o.converged = snap.is_some_and(|s| {
                        s.joined && accepted.iter().all(|m| *m == o.mac || s.links.iter().any(|l| l.peer == *m))
                    });
                }
            }
            Err(e) => eprintln!("warning: poll: {e:#}"),
        }
        if outcomes.iter().all(|o| !o.accepted || o.converged) || Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    Ok(Report { nid: plan.nid, outcomes, elapsed: started.elapsed(), interrupted: stop.load(Ordering::SeqCst) })
}

pub fn rekey(cfg: &Config, args: &RekeyArgs) -> Result<()> {
    if cfg.interfaces.len() != 1 {
        return refused("rekey needs exactly one interface: pass -i or set interfaces in the config".to_string());
    }
    let passphrase = read_passphrase(args.passphrase_stdin)?;
    let (min, max) = if args.allow_short { (8, 63) } else { (24, 64) };
    validate_passphrase(&passphrase, min, max)?;
    let net = poll_once(cfg)?;
    let plan = plan(cfg, &net, &passphrase, &args.only, &args.config_path)?;
    print!("{}", plan_text(&plan));
    if args.dry_run {
        let frame = set_nmk_request(Mac([0; 6]), plan.steps[0].mac, &plan.nmk).encode()?;
        println!("Dry run: the set-NMK request is MMTYPE 0xa018, {} bytes encoded, sent unicast to each adapter above in turn. Nothing was sent.", frame.len());
        return Ok(());
    }
    if !args.yes && !confirm(plan.steps.len())? {
        println!("Aborted. Nothing was written.");
        exit(5);
    }
    println!(
        "Writing the new key to {} adapter(s), then waiting up to {} s for them to rejoin on NID {} ...",
        plan.steps.len(),
        args.timeout_seconds,
        plan.nid
    );
    let report = execute(cfg, &plan, Duration::from_secs(args.timeout_seconds))?;
    for (i, step) in plan.steps.iter().enumerate() {
        let line = match report.outcomes.get(i) {
            None => "not attempted".to_string(),
            Some(o) => match (&o.error, o.converged) {
                (Some(e), false) => format!("FAILED: {e}"),
                (Some(e), true) => format!("{e}, but it now reports the new NID"),
                (None, true) => "set key accepted, rejoined on the new NID".to_string(),
                (None, false) => match o.status {
                    Some(JoinStatus::HasKeyNotJoined) => format!("set key accepted, has the new key, still joining after {} s", args.timeout_seconds),
                    Some(st) => format!("set key accepted, {st} after {} s", args.timeout_seconds),
                    None => format!("set key accepted, not seen on the new NID within {} s", args.timeout_seconds),
                },
            },
        };
        println!("  {}  {:<18}  {line}", i + 1, step.name);
    }
    let done = report.outcomes.iter().filter(|o| o.converged).count();
    let secs = report.elapsed.as_secs();
    if report.interrupted && report.outcomes.is_empty() {
        println!("Interrupted before any write. Nothing was written.");
        exit(130);
    }
    let code = match report.outcomes.iter().position(|o| !o.accepted) {
        _ if report.interrupted => 7,
        Some(0) => 6,
        Some(_) => 7,
        None if done == plan.steps.len() => 0,
        None => 2,
    };
    if code == 0 {
        println!("OK  Rekeyed {done} of {} in {secs} s; all report NID {}.", plan.steps.len(), plan.nid);
        return Ok(());
    }
    if report.interrupted {
        println!("Interrupted by Ctrl-C after {} write(s).", report.outcomes.len());
    }
    println!(
        "FAILED  {done} of {} adapters confirmed on NID {} after {secs} s; the network may be split across keys.\n\
         Recovery: re-run `powerline-monitor rekey` with the previous passphrase (use --only <mac> for the adapters that moved);\n\
         or press Pair (1 s) on an adapter already on the wanted key, then within 2 min on each stray one;\n\
         or hold Pair 6 s+ on every adapter for a factory reset back to \"HomePlugAV\".",
        plan.steps.len(),
        plan.nid
    );
    exit(code);
}
