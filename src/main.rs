mod commands;

use std::path::PathBuf;
use std::process::exit;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use powerline_core::client::{Client, Unreachable};
use powerline_core::config::{Config, ResolveError};
use powerline_core::events::Severity;
use powerline_core::model::NetState;
use powerline_core::poll::poll_once;
use powerline_core::render;
use powerline_core::sock::SockError;
use powerline_ops::Refused;

#[derive(Parser)]
#[command(name = "powerline-monitor", version, about = "Monitor HomePlug AV powerline adapters: status, link speeds, history, events, Prometheus metrics")]
struct Cli {
    /// Config file (default: $XDG_CONFIG_HOME/powerline-monitor/config.toml, else /etc/powerline-monitor.toml)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Interface to use, repeatable; overrides `interfaces` from the config
    #[arg(short, long, global = true, value_name = "NAME")]
    interface: Vec<String>,
    /// Read from a running powerline-monitor daemon at this URL instead of polling the adapters; overrides daemon_url from the config
    #[arg(long, global = true, value_name = "URL")]
    daemon: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the adapters seen on each interface
    Discover {
        /// Print JSON instead of the table
        #[arg(long)]
        json: bool,
        /// Print a [devices] block for every adapter not yet in the config, ready to append to it
        #[arg(long)]
        toml: bool,
    },
    /// One poll: verdict, adapters and link speeds; exit 2 on a critical finding, 1 on a warning
    Status {
        /// Print JSON instead of the text view
        #[arg(long)]
        json: bool,
        /// Add protocol detail: NID, SNID, TEI, interface, signal level and link flags
        #[arg(short, long)]
        verbose: bool,
    },
    /// The status view redrawn in the terminal after every poll, with rate sparklines
    Watch(powerline_watch::Args),
    /// Poll continuously and serve Prometheus metrics and JSON state over HTTP
    Daemon(powerline_daemon::Args),
    /// Open the desktop window (the default when no command is given)
    Ui(powerline_ui::Args),
    /// Write a new network key to the configured adapters
    Rekey(powerline_ops::RekeyArgs),
    /// Report whether the network runs on the factory passphrase HomePlugAV
    CheckDefaultKey(powerline_ops::CheckArgs),
    /// Link speed history from the daemon or its saved file: every link, or an hourly grid for one pair
    History(commands::HistoryArgs),
    /// Events recorded by the daemon
    Events(commands::EventsArgs),
    /// Give an adapter a local name in the config; the adapter itself is not written
    Rename(commands::RenameArgs),
    /// Validate or show the config
    Config {
        #[command(subcommand)]
        cmd: commands::ConfigCmd,
    },
    /// One poll as Prometheus text, for node_exporter's textfile collector
    Export,
    /// Print a shell completion script
    Completions { shell: clap_complete::Shell },
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { 4 } else { 0 };
            e.print().ok();
            exit(code);
        }
    };
    if !matches!(cli.cmd, None | Some(Cmd::Daemon(_)) | Some(Cmd::Ui(_))) {
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    match &cli.cmd {
        Some(Cmd::Completions { shell }) => {
            clap_complete::generate(*shell, &mut Cli::command(), "powerline-monitor", &mut std::io::stdout());
            return;
        }
        Some(Cmd::Config { cmd: commands::ConfigCmd::Validate { file } }) => {
            let path = file.clone().unwrap_or_else(|| commands::config_file(cli.config.as_deref()));
            exit(commands::validate(&path));
        }
        _ => {}
    }
    let (file_cfg, path) = match Config::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            exit(4);
        }
    };
    let mut cfg = file_cfg.clone();
    if !cli.interface.is_empty() {
        cfg.interfaces = cli.interface.clone();
    }
    if cli.daemon.is_some() {
        cfg.daemon_url = cli.daemon.clone();
    }
    let result = match cli.cmd.unwrap_or(Cmd::Ui(Default::default())) {
        Cmd::Discover { json, toml } => fetch(&cfg).and_then(|s| {
            if toml {
                print!("{}", render::discover_toml(&s, &cfg));
                Ok(())
            } else {
                show(&s, &cfg, json, render::discover_table)
            }
        }),
        Cmd::Status { json, verbose } => fetch(&cfg).and_then(|s| {
            show(&s, &cfg, json, |s, c| render::status_text(s, c, None, verbose))?;
            match render::verdict(&s, &cfg, None).severity {
                Some(Severity::Critical) => exit(2),
                Some(_) => exit(1),
                None => Ok(()),
            }
        }),
        Cmd::Watch(a) => powerline_watch::run(&cfg, &a),
        Cmd::Daemon(a) => powerline_daemon::run(&cfg, &path, &a),
        Cmd::Ui(a) => powerline_ui::run(cfg, file_cfg, path, a),
        Cmd::Rekey(mut a) => {
            a.config_path = path.clone();
            powerline_ops::rekey(&cfg, &a)
        }
        Cmd::CheckDefaultKey(a) => powerline_ops::check_default_key(&cfg, &a),
        Cmd::History(a) => commands::history(&cfg, &a),
        Cmd::Events(a) => commands::events(&cfg, &a),
        Cmd::Rename(a) => commands::rename(&cfg, &file_cfg, &path, &a),
        Cmd::Config { cmd: commands::ConfigCmd::Show } => commands::show_config(&cfg),
        Cmd::Export => commands::export(&cfg),
        Cmd::Completions { .. } | Cmd::Config { .. } => unreachable!(),
    };
    match result {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {e:#}");
            let code = if e.downcast_ref::<SockError>().is_some()
                || e.downcast_ref::<Unreachable>().is_some()
                || e.downcast_ref::<ResolveError>().is_some()
            {
                4
            } else if e.downcast_ref::<Refused>().is_some() {
                5
            } else {
                1
            };
            exit(code);
        }
    }
}

/// The daemon's last state when a daemon URL is set, else one local poll.
fn fetch(cfg: &Config) -> Result<NetState> {
    let Some(url) = &cfg.daemon_url else { return poll_once(cfg) };
    let client = Client::new(url);
    match client.state()? {
        Some(s) => Ok(s),
        None => {
            eprintln!("daemon at {} has not polled yet", client.url());
            exit(3);
        }
    }
}

fn show(state: &NetState, cfg: &Config, json: bool, text: impl Fn(&NetState, &Config) -> String) -> Result<()> {
    if state.devices.is_empty() {
        eprintln!("no adapters found on {}", cfg.effective_interfaces().join(", "));
        exit(3);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(state)?);
    } else {
        print!("{}", text(state, cfg));
    }
    Ok(())
}
