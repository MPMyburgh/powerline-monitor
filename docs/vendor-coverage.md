# Coverage of TP-Link's tpPLC utility

What tpPLC does and where powerline-monitor stands on the TL-PA7017.
"Needs a write" rows are not implemented: each needs an allowlist change and
a supervised first send on hardware where that write has not been verified.

| tpPLC feature | powerline-monitor | How |
|---|---|---|
| Discover local and remote devices, Refresh | done | 0xA070 broadcast, then a unicast walk of every station the answering adapters list |
| Network map: which adapter is wired here, roles | done | 0xA028 and `CC_DISCOVER_LIST`; the tree in the window, `discover`, `status` |
| Per-link TX/RX rates, coloured | done | 0xA02C; Speeds tab, `status`, `watch`, `history`, `/metrics` |
| Device info, firmware version | done | 0xA04C, `CM_STA_CAP`, `CM_STA_IDENTIFY`; Adapter tab, `status` |
| Clients behind each adapter | done | `CM_BRG_INFO`; Adapter tab, `/api/state`, `homeplug_bridged_stations` |
| Check the network key | done without reading keys | `check-default-key` compares NIDs |
| Rename a device | done locally | the name is stored in the config. Writing the adapter's own name (0xA058 property 0x25) needs a write |
| Set a new network name (Secure) | done as `rekey` | 0xA018 per adapter, remotes first; the only write in the binary |
| Add a device by its powerline key (DAK) | needs a write (0xA010) | not implemented |
| Remove a remote device | needs a write (no published MME) | not implemented |
| Pair button emulation | no Broadcom MME is published | not implemented |
| LEDs on/off | needs a write (0xA058 properties 0x95 and 0x3F, then 0xA020 restart) | not implemented |
| LED state | read-only possible (0xA05C property 0x3F) | not implemented: no property reads |
| Power saving on/off | needs a write (0xA058 property 0x29, then 0xA020) | not implemented |
| Power saving state | read-only possible (0xA05C property 0x29) | not implemented: no property reads |
| QoS traffic type | needs a write (0xA058 property 0x69) | not implemented |
| Reset to factory defaults | needs a write (0xA054) | not implemented |
| Restart | needs a write (0xA020) | not implemented |
| Link rates via the standard `CM_NW_STATS` (0x6048) | redundant, not sent | remote adapters answer, the wired one does not; 0xA02C covers all |
| Beacon via `CM_GET_BEACON` (0x603C) | redundant, not sent | |
| PLC-to-VDSL interference mode | not applicable | 8- and 9-series adapters only |
| Extender Wi-Fi settings, usage tips, firmware update | out of scope | |
