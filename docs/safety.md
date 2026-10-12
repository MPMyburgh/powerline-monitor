# Safety model

## What it does not do

- It does not flash firmware, write PIBs or NVM, read or write device memory,
  change LEDs or power saving, factory-reset or restart adapters. Those
  message types are not in the send allowlist and cannot be added by
  configuration.
- It never reads, prints or stores the network key (NMK), the device access
  keys (DAK) or a passphrase. The factory-key check compares network IDs,
  which are derived from the key.
- It is not a long-term time-series store. The daemon keeps `history_hours`
  of readings in a ring that survives restarts; Prometheus keeps the durable
  record.

## Send allowlist

The frame encoder refuses any management message that is not on a
compiled-in list keyed on (ethertype, OUI, MMTYPE). This is the whole of what
the binary can send:

| Ethertype | OUI | MMTYPE | Purpose |
|---|---|---|---|
| 0x8912 | 00:1f:84 (Broadcom) | 0xA070 | discover, broadcast and unicast |
| 0x8912 | 00:1f:84 | 0xA028 | network info: NID, SNID, TEI, role, coordinator |
| 0x8912 | 00:1f:84 | 0xA02C | network stats: PHY rate and link flags per peer |
| 0x8912 | 00:1f:84 | 0xA04C | station info: firmware version, uptime |
| 0x8912 | 00:1f:84 | 0xA018 | set network key: the one write, used by `rekey` only |
| 0x88E1 | none | 0x0014 | `CC_DISCOVER_LIST`: stations heard, signal level, other networks |
| 0x88E1 | none | 0x6020 | `CM_BRG_INFO`: MACs bridged behind the adapter |
| 0x88E1 | none | 0x6034 | `CM_STA_CAP`: AV version, OUI, capabilities |
| 0x88E1 | none | 0x6060 | `CM_STA_IDENTIFY`: power-save capability, carrier range |

Property reads and writes (0xA05C/0xA058), restart (0xA020), factory reset
(0xA054), DAK authorisation (0xA010), `CM_SET_KEY`, firmware, flash, memory
and PIB messages are not on the list, and there is no flag to add them. The
list lives in `crates/core/src/frame.rs`.

## Reads

Replies are matched on source MAC, ethertype, OUI, message type and sequence
number, so stray frames from other tools on the same wire are ignored. Replies
are parsed with length checks; a short reply is an error, not zeros.

No property reads are made at all. The NMK and DAK properties are never
requested, and no key or passphrase is printed, logged, returned over the API
or written to disk. `rekey` derives the new NID from the passphrase and
prints that.

## Writes

Only `rekey` writes. It is unicast to one adapter at a time over exactly one
interface, targets only adapters in the config (or named with `--only`),
prints the plan first, has a `--dry-run` that sends nothing, and otherwise
writes only after you type `REKEY` on the controlling terminal, which a pipe
on stdin cannot supply. `--yes` skips that question for scripted use. The
passphrase comes from a prompt or stdin, never from argv.

## Daemon

The HTTP API has no authentication. `/api/state` carries the LAN MACs bridged
behind each adapter and the config's names, so `listen` stays on loopback or
a VPN interface.

## Privileges

The binary carries `CAP_NET_RAW` as a file capability. The system unit runs
as a `DynamicUser` with `CAP_NET_RAW` as its whole capability bounding set
and its state directory as the only writable path.
