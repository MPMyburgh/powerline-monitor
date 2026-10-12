# Configuration

TOML, every key optional; the file may be absent. Flags override the file.
Lookup: `--config PATH`, else `$XDG_CONFIG_HOME/powerline-monitor/config.toml`
(`~/.config/powerline-monitor/config.toml`), else `/etc/powerline-monitor.toml`.
Unknown keys are errors. `packaging/powerline-monitor.toml.example` is a
commented copy; `config show` prints the effective values.

```toml
interfaces    = []                 # empty: every UP interface with /sys/class/net/<name>/type == 1, in name order.
                                   # rekey needs exactly one, from here or from -i
poll_seconds  = 30                 # 5..=600; the daemon and the window (watch has --interval-seconds)
history_hours = 24                 # 1..=720; ring capacity = history_hours * 3600 / poll_seconds readings per link
listen        = "127.0.0.1:9702"   # where the daemon serves /metrics and /api/*
# daemon_url  = "http://server:9702"   # read from this daemon instead of polling (the --daemon flag)
notify_min_severity = "warning"    # "info" | "warning" | "critical" | "off"; reserved for desktop
                                   # notifications, which 0.1 does not send yet

[alerts]                           # when an event is worth reporting
lost_after_polls     = 3           # 1..=20   an adapter is offline after this many missed polls in a row
degraded_after_polls = 10          # 1..=100  a link is slow after staying slow for this many polls in a row
floor_mbps           = 50          # 1..=2000 a link is slow below this rate, whatever it usually does
drop_pct             = 40          # 1..=90   ...or when it drops this far below its usual rate (ring median)

[hooks]
exec         = []                  # program and fixed arguments, run per event; a single string is accepted
min_severity = "warning"           # lowest severity handed to the hook

[devices."20:e1:5d:0a:1b:2c"]      # one table per adapter you own, keyed by MAC (':' or '-' separators)
alias  = "Study"                   # 1..=32 printable characters, unique, not MAC-like, not "Adapter XXXX"
vendor = "broadcom"                # "broadcom" | "qualcomm": overrides the OUI probe

[spares]                           # adapters you own but keep unplugged: named when they appear,
"20:e1:5d:aa:bb:cc" = "Drawer spare"   # never reported as unknown; a MAC cannot be in both tables
```

An adapter in `[devices]` or `[spares]` counts as yours; any other adapter on
the network is an unknown station (critical). Unknown-station detection is
off while `[devices]` is empty. An unnamed adapter is shown as `Adapter 1B2C`
(the last four hex digits of its MAC) everywhere.

## Saving

`rename` and the window's Settings tab edit the file in place: comments and
keys they do not own survive, values are written only where they differ, a
key still at its default is not added, and a missing file starts from the
packaged example.

## State directory

`$STATE_DIRECTORY` (set by systemd), else `$XDG_STATE_HOME/powerline-monitor`,
else `~/.local/state/powerline-monitor`, created mode 0700 by the daemon.

| File | Contents |
|---|---|
| `history.json` | the reading ring; saved every 10 polls and at shutdown, reloaded and resized to the config at start |
| `tracker.json` | the event tracker (miss and slow counters, degraded links, heard networks); saved whenever an event fires, so a restart re-fires nothing |
| `events.jsonl` | one hook payload per line, append-only; what `events` reads when no daemon answers |
