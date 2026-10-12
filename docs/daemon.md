# Daemon, HTTP API and Prometheus

`powerline-monitor daemon` prints a banner (version, config path, state
directory, interfaces, interval, metrics URL, hook), then one line per poll.
Every event is printed to stderr as its JSON payload, so journald keeps
them. SIGTERM or Ctrl-C saves the ring and tracker and exits 0.

## Endpoints

| Endpoint | Response |
|---|---|
| `GET /metrics` | Prometheus text |
| `GET /api/state` | the last poll as JSON (`NetState`); `503 {"error":"no poll yet"}` until the first poll completes |
| `GET /api/history` | the reading ring as JSON, the same shape as `history.json` |
| `GET /api/history.csv` | every reading as `time,from,from_name,to,to_name,tx_mbps,rx_mbps` |
| `GET /api/events` | the last 500 events as `[{kind, at, ...}]`; `?since=30m`, `6h`, `2d`, unix seconds or RFC 3339 filters them |
| `GET /api/status` | `{version, interfaces, poll_seconds, last_poll_at, last_poll_ok, polls, poll_duration, state_dir, hook}` |
| `GET /healthz` | `ok` |

There is no authentication, and `/api/state` lists the LAN MACs bridged
behind each adapter, so keep `listen` on loopback or a VPN interface
(`config validate` warns otherwise).

Commands and the window reach a daemon through `--daemon URL` or
`daemon_url`: `powerline-monitor status --daemon http://127.0.0.1:9702`
prints the daemon's last poll with the same verdict and exit code.
`POWERLINE_MONITOR_HOST` in the environment sets the `host` field of every
payload (the units set it to the machine name); otherwise the kernel
hostname is used.

## Events

| Kind | Severity | Fired when |
|---|---|---|
| `nmk.default` | critical | the network's NID derives from `HomePlugAV` |
| `station.unknown` | critical | an adapter in neither `[devices]` nor `[spares]` is seen (once `[devices]` has an entry) |
| `station.gone` | info | that adapter has been missing for `lost_after_polls` polls |
| `adapter.lost` | warning | a named adapter has not answered for `lost_after_polls` polls |
| `adapter.seen` | info | it answers again |
| `adapter.rebooted` | warning | an adapter's uptime went backwards |
| `cco.changed` | warning | another adapter coordinates the network |
| `nmk.changed` | warning | the NID changed (a rekey or a pairing) |
| `link.degraded` | warning | a direction stayed under `floor_mbps`, or `drop_pct` below its ring median (once the ring holds 20 readings), for `degraded_after_polls` polls |
| `link.recovered` | info | it stayed above that for `degraded_after_polls` polls |
| `network.heard` | warning; critical when that network is on the factory key and yours is not | another NID is heard on the wiring |
| `poller.blind` | warning | `lost_after_polls` polls in a row failed or found nothing |
| `poller.ok` | info | polls work again |

The first successful poll reports only `nmk.default`, `station.unknown` and
`network.heard`; presence and link events need history.

## Hook

With `[hooks] exec` set, the daemon runs the program once per event whose
severity is at least `hooks.min_severity`, one at a time:

- argv: `exec[0]`, `exec[1..]`, then the event kind (for example
  `link.degraded`) as the last argument;
- environment: `POWERLINE_MONITOR_EVENT` (the kind),
  `POWERLINE_MONITOR_SEVERITY` (`info`/`warning`/`critical`),
  `POWERLINE_MONITOR_SUMMARY` (the sentence);
- stdin: the payload as one JSON object;
- 10 s to finish, then it is killed; a non-zero exit or timeout is logged,
  not retried.

The payload, as written to stdin, to `events.jsonl` and by `events --json`:

```json
{"advice":"Fix it in a terminal: powerline-monitor rekey --dry-run first","detail":{"nid":"b0:f2:e6:95:66:6b:03"},"host":"desktop","nid":"b0:f2:e6:95:66:6b:03","severity":"critical","subject":null,"summary":"The powerline network is on the factory password; anyone's adapter on this wiring can join","time":"2026-10-08T00:05:39+02:00","type":"nmk.default"}
```

`subject` is `{"mac","name"}` for adapter events, `{"tx","tx_name","rx","rx_name"}`
for link events, else null. `detail` holds the kind's own fields (`was`/`now`
rates or MACs, `after_secs`, `was_up_secs`/`now_up_secs`, `error`). `advice`
is the sentence the window shows under the event, or null.

```toml
[hooks]
exec = ["/usr/local/bin/plc-alert", "--room", "home"]
min_severity = "warning"
```

## Prometheus

```yaml
scrape_configs:
  - job_name: powerline
    static_configs:
      - targets: ["server:9702"]
```

Namespace `homeplug`. The three series shared with homeplug_exporter keep its
names and labels so existing dashboards port.

| Metric | Labels | Meaning |
|---|---|---|
| `homeplug_station_tx_rate_bytes` | `device_addr, nid, peer_addr` | PHY TX rate from device to peer, bytes/s (Mbit/s × 125 000) |
| `homeplug_station_rx_rate_bytes` | `device_addr, nid, peer_addr` | PHY RX rate, bytes/s |
| `homeplug_network_info` | `device_addr, nid, snid, tei, role, cco_addr, cco_tei` | always 1; role is `STA`, `CCO`, `BACKUP_CCO`, `UNASSOC_STA`, `UNASSOC_CCO` or `UNKNOWN` |
| `homeplug_station_link_info` | `device_addr, peer_addr, spec, mimo, beamforming` | always 1; spec `av11`/`av20`/`ieee1901`, mimo `siso1`/`siso2`/`mimo`/`siso_only` |
| `homeplug_station_signal_level` | `device_addr, peer_addr` | signal band heard from the peer: 1 strongest, 15 weakest, 0 unknown |
| `homeplug_device_info` | `device_addr, interface, vendor, hfid, local, alias, firmware, oui, av` | always 1; `alias` is the display name |
| `homeplug_device_up` | `device_addr` | 1 if the adapter answered the last poll; named adapters that did not answer are listed with 0 |
| `homeplug_device_uptime_seconds` | `device_addr` | seconds since the adapter booted |
| `homeplug_bridged_stations` | `device_addr` | Ethernet stations bridged behind the adapter |
| `homeplug_unknown_stations` | | adapters in neither `[devices]` nor `[spares]`; 0 while `[devices]` is empty |
| `homeplug_networks_heard` | | other powerline networks heard on the wiring |
| `homeplug_network_default_key` | `nid` | 1 if the NID derives from `HomePlugAV` |
| `homeplug_default_key` | | 1 if the network's majority NID derives from `HomePlugAV` |
| `homeplug_up` | | 1 if the last poll succeeded |
| `homeplug_polls_total` | | polls attempted |
| `homeplug_events_total` | `kind` | events recorded since start, by kind |
| `homeplug_poll_duration_seconds` | | length of the last poll |
| `homeplug_last_poll_timestamp_seconds` | | when the last poll finished |

Example queries and alert rules:

```promql
# PHY rate per directed link in Mbit/s
homeplug_station_tx_rate_bytes * 8 / 1e6

# the same, with the configured name as a legend label
homeplug_station_tx_rate_bytes * on(device_addr) group_left(alias) homeplug_device_info

# alert rules worth having on day one
homeplug_device_up == 0
homeplug_network_default_key == 1
homeplug_unknown_stations > 0
time() - homeplug_last_poll_timestamp_seconds > 120

# a link running at under 60% of its own 7-day median
homeplug_station_tx_rate_bytes < 0.6 * quantile_over_time(0.5, homeplug_station_tx_rate_bytes[7d])
```

Port 9702 and the shared series names are homeplug_exporter's; do not run
both on one host. Rates here are SI (Mbit/s × 125 000) where
homeplug_exporter used × 131 072, so ported panels read 4.6% lower.
