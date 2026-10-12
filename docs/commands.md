# Command reference

```
powerline-monitor [--config PATH] [-i IFACE]... [--daemon URL] [<command>]
```

| Flag | Meaning |
|---|---|
| `--config PATH` | Config file. Default: `$XDG_CONFIG_HOME/powerline-monitor/config.toml`, else `/etc/powerline-monitor.toml`. A path given here must exist. |
| `-i, --interface IFACE` | Interface to use, repeatable; overrides `interfaces` from the config. |
| `--daemon URL` | Read from a running daemon instead of polling the adapters; overrides `daemon_url`. `http://host:port` or `host:port`. |

The flags are global and go before or after the command. With no command the
window opens. `powerline-monitor <command> --help` describes each flag.

An `ADAPTER` argument is a name from the config (case-insensitive), a full
MAC, or the last four or more hex digits of a MAC when they are unique
(`1b2c`, `0a:1b:2c`).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Done. For `status` and `check-default-key`: nothing wrong. |
| 1 | `status`: a warning (an adapter missing, a slow link). Other commands: an unexpected error. |
| 2 | A critical finding: `status` (factory password or unknown adapter), `check-default-key` FAIL, or a `rekey` whose adapters did not all rejoin. |
| 3 | Nothing to show: no adapter answered, the daemon has not polled yet, or no history or events exist here. |
| 4 | Usage, config or environment error: bad flag or config, unknown adapter name, no `CAP_NET_RAW`, interface missing or down, daemon unreachable. |
| 5 | Refused, nothing written: a `rekey` preflight failed or you did not type `REKEY`. |
| 6 | The first key write failed; the network is unchanged. |
| 7 | A later key write failed, or Ctrl-C arrived after the first write; the network may be split across keys. Recovery steps are printed. |
| 130 | `rekey` interrupted before any write. |

## discover

One row per adapter seen on each interface, then the network line. Exit 3
when none answered.

```
$ powerline-monitor discover
enp0s31f6: 1 adapter wired to this computer, 2 more over the powerline
NAME          MAC                WHERE       ROLE                FIRMWARE
Adapter 1B2C  20:e1:5d:0a:1b:2c  wired here  backup coordinator  3.2.4 (svn 14077)
Adapter 3D4E  20:e1:5d:0a:3d:4e  powerline   coordinator         3.2.4 (svn 14077)
Adapter 5F60  20:e1:5d:0a:5f:60  powerline   member              3.2.4 (svn 14077)

Network b0:f2:e6:95:66:6b:03: 3 adapters, 3 not named.
Factory password in use: see powerline-monitor check-default-key
```

`--toml` prints a `[devices]` table for every adapter not yet in the config,
ready to append (`powerline-monitor discover --toml >> ~/.config/powerline-monitor/config.toml`).
`--json` prints the poll as JSON, the same `NetState` as `status --json`.

## status

A verdict line, the network, the adapter table, the link-rate matrix (row
sends to column; `~` means the receiver's figure was used, `!` marks a rate
under `alerts.floor_mbps`), the links slowest first, and footer notes.

```
$ powerline-monitor status
CRITICAL  Factory password in use. 3 of 3 adapters online, links normal.

Network: 3 adapters · coordinator Adapter 3D4E · backup Adapter 1B2C
NAME          ROLE                WHERE                   FIRMWARE           UP
Adapter 1B2C  backup coordinator  wired to this computer  3.2.4 (svn 14077)  49 d 08 h
Adapter 3D4E  coordinator         over the powerline      3.2.4 (svn 14077)  49 d 08 h
Adapter 5F60  member              over the powerline      3.2.4 (svn 14077)  3 d 13 h

Link speed, Mbit/s (row sends to column; real throughput is roughly half)
                Adapter 1B2C  Adapter 3D4E  Adapter 5F60
Adapter 1B2C               -           514           389
Adapter 3D4E             526             -           475
Adapter 5F60             463           509             -

LINKS, slowest first
Adapter 1B2C ↔ Adapter 5F60  389 / 463  ok
Adapter 3D4E ↔ Adapter 5F60  475 / 509  ok
Adapter 1B2C ↔ Adapter 3D4E  514 / 526  ok

Factory password in use: see powerline-monitor check-default-key
No adapters named in the config, so unknown-adapter detection is off
```

The verdict is decided in this order: an adapter not in the config (critical,
only once `[devices]` has an entry), the factory password (critical), a named
adapter that did not answer (warning), a link under `alerts.floor_mbps` or
flagged degraded by the daemon (warning), else OK.

`-v` adds the NID, SNID and security level to the network line, and MAC, TEI,
interface, signal level and link type columns. `--json` prints the poll as
JSON with every NID as colon-hex and every timestamp as RFC 3339 with the
local offset.

## watch

The `status` view redrawn after every poll (default every 5 s), then one row
per pair of adapters, slowest first, with both directions, the lowest and
usual (median) rate since `watch` started, and a block sparkline of the
slower direction (one block per 75 Mbit/s, lowest reading per bucket).
Keys: `q` or Ctrl-C quit, `r` poll now, `p` pause, `+`/`-` move the interval
by 5 s.

```sh
powerline-monitor watch --interval-seconds 10
```

When stdout is not a terminal, or with `--lines`, it prints one line per poll
for `tee` and `grep`:

```
$ powerline-monitor watch --lines
00:07:54  1B2C>5F60 391  1B2C>3D4E 505  3D4E>5F60 467  3D4E>1B2C 525  5F60>3D4E 509  5F60>1B2C 475
```

`--json-lines` prints one compact `NetState` per poll.

## ui

The desktop window. Tabs: Overview, Speeds, History, Events, Settings,
Adapter. The left panel holds the naming card (until every adapter has a
name) and the network tree with roles and a pencil to rename in place. The
window title carries the verdict so it reads from the task bar. Settings
edits the config file and saves it with comments intact.

Keys: `1`-`6` switch tabs, `R` refresh, `D` technical details, `/` focus the
event filter, `Space` pause polling, `←`/`→` move between links on the
History tab, `Esc` cancel a rename, `Q` quit.

| Flag | Meaning |
|---|---|
| `--tab TAB` | `overview`, `speeds`, `history`, `events`, `adapter` or `settings` |
| `--select FROM,TO` | Preselect a directed link as `from_mac,to_mac` (default: the slowest link) |
| `--adapter NAME\|MAC` | Open the Adapter tab on this adapter |
| `--screenshot PATH` | Save a PNG of the window after the first poll, then exit |
| `--size WxH` | Window size (default 1100x700, or the last size used) |

Window state lives in `~/.local/share/powerline-monitor/app.ron`. When a
daemon answers on `listen` the window seeds its history and events from it;
with `--daemon URL` it attaches to that daemon, polls nothing itself and
needs no `CAP_NET_RAW`.

## daemon

Polls every `poll_seconds`, keeps `history_hours` of readings, tracks events,
runs the hook and serves HTTP on `listen` (`--listen ADDR` overrides). Scrapes
read the last snapshot and never touch the adapters. See
[daemon.md](daemon.md).

## history

Link-speed history from the daemon (`--daemon URL`, `daemon_url`, else the
daemon on `listen`), else from `history.json` in the state directory. Exit 3
with nothing to read. `--since` takes `30m`, `6h`, `2d`, unix seconds or
RFC 3339 (default `24h`).

```
$ powerline-monitor history
Link speed, Mbit/s, since Yesterday 00:05 (3 readings)

LINK                          NOW  LOWEST  USUAL  READINGS
Adapter 1B2C → Adapter 5F60   395     395    395         3  ▆▆▆
Adapter 5F60 → Adapter 1B2C   463     463    463         3  ▇▇▇
```

With two adapters it keeps that pair and adds a 24-row hourly grid of the
usual speed in each direction, plus a "lower than usual since" note when the
last hour's median sits under the link's 10th percentile:

```sh
powerline-monitor history 1b2c 3d4e
```

`--csv` prints every reading as `time,from,from_name,to,to_name,tx_mbps,rx_mbps`;
`--json` prints the rows and, for a pair, the hourly grid.

## events

Events from the daemon, else from `events.jsonl` in the state directory;
exit 3 with neither.

```
$ powerline-monitor events
00:05  critical  nmk.default       The powerline network is on the factory password; anyone's adapter on this wiring can join
```

`--since WHEN` as for `history`, `--severity info|warning|critical` is the
lowest level shown, `--json` prints the hook payload one per line, `--follow`
keeps printing new events.

## rename

Writes a name into the config under `[devices."mac"]`. The adapter itself is
not written. When the config alone cannot resolve the adapter, one poll is
made to find it.

```
$ powerline-monitor rename 1b2c Study
Named 20:e1:5d:0a:1b:2c "Study" in ~/.config/powerline-monitor/config.toml. This is a local name; the adapter itself is not written.
```

Names are 1 to 32 printable characters, unique, not MAC-like and not a
default `Adapter XXXX`. `--clear` removes the name. The file is edited in
place; comments and unrelated keys survive.

## config validate, config show

`config validate [FILE]` checks TOML syntax, unknown keys, ranges, name rules
and a MAC listed in both `[devices]` and `[spares]`; exit 4 on errors, each
with a line number and a suggestion where one fits:

```
error: ~/bogus.toml:12: unknown field `pol_seconds`; did you mean `poll_seconds`?
```

Warnings (exit 0): `listen` not on loopback, an interface not present on this
computer, `hooks.exec[0]` not an executable file.

`config show` prints the effective config as TOML with every default filled
in and `-i`/`--daemon` applied.

## export

One poll rendered as the same Prometheus text the daemon serves, for
node_exporter's textfile collector. Needs `CAP_NET_RAW`.

```sh
powerline-monitor export > powerline.prom
```

## check-default-key

Reports whether the network runs on the factory passphrase `HomePlugAV`. It
derives the network ID from that passphrase and compares it with the one each
adapter reports; no key leaves an adapter. Both security levels of the
factory NID are matched. Exit 2 on FAIL, 0 on PASS, 3 when no adapter
answered; `--json` gives the per-adapter result.

```
$ powerline-monitor check-default-key
ALIAS  MAC                NID                   NID IS DEFAULT
-      20:e1:5d:0a:1b:2c  b0:f2:e6:95:66:6b:03  yes
-      20:e1:5d:0a:3d:4e  b0:f2:e6:95:66:6b:03  yes
-      20:e1:5d:0a:5f:60  b0:f2:e6:95:66:6b:03  yes

FAIL  3 of 3 adapters use the factory key. Any HomePlug AV adapter on this
      wiring, yours or a neighbour's, joins your LAN without pairing.
      Fix: powerline-monitor rekey
```

## rekey

The one command that writes to the adapters. Run the dry run first:

```sh
powerline-monitor rekey --dry-run
```

It asks for the passphrase twice without echo (24 to 64 printable ASCII
characters; `--allow-short` accepts 8 to 63; `--passphrase-stdin` reads one
line from stdin for scripts), polls, prints the plan and sends nothing:

```
Rekey plan: 3 adapter(s) move to NID 7c:3e:91:0b:5d:a2:02 (SL-SC). Remote adapters go first over the powerline (the CCo last among them), the local one last; adapters on different keys cannot see each other until all have moved. Nothing has been written.
#  ALIAS    MAC                ROLE  VIA    CURRENT NID           NEW NID
1  Bedroom  20:e1:5d:0a:5f:60  STA   plc    b0:f2:e6:95:66:6b:03  7c:3e:91:0b:5d:a2:02
2  Lounge   20:e1:5d:0a:3d:4e  CCo   plc    b0:f2:e6:95:66:6b:03  7c:3e:91:0b:5d:a2:02
3  Study    20:e1:5d:0a:1b:2c  bCCo  local  b0:f2:e6:95:66:6b:03  7c:3e:91:0b:5d:a2:02
Dry run: the set-NMK request is MMTYPE 0xa018, 60 bytes encoded, sent unicast to each adapter above in turn. Nothing was sent.
```

Preflight refusals (exit 5, nothing written): more or fewer than one
interface; the factory passphrase itself; every target already on the new
NID; any Broadcom adapter on the wire that is neither in `[devices]`/`[spares]`
nor named with `--only MAC`.

`--only MAC` (repeatable) restricts the write to those adapters and is the
only way to write an adapter that is not in the config; it is also the
recovery path when a rekey leaves the network split. Without `--dry-run` the
tool asks you to type `REKEY` on the controlling terminal (`--yes` skips the
question), writes the key to each adapter in plan order, waits up to
`--timeout-seconds` (default 60) for every adapter to rejoin on the new NID,
and prints the per-adapter outcome. Ctrl-C after the first write stops
further writes and prints the recovery steps.

Remote rekey over the powerline is verified only as far as `--dry-run` on the
TL-PA7017. For a first live run, use `--only` with one remote adapter.

## completions

```sh
powerline-monitor completions fish > ~/.config/fish/completions/powerline-monitor.fish
```

`install.sh` and the Arch package do this for bash, zsh and fish.
