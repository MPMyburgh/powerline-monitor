# Design notes

Decisions recorded so they are not reopened by accident.

- The desktop window is egui/eframe; `watch` is the terminal view. There is
  no separate TUI.
- NIDs are wire-order colon-hex (`b0:f2:e6:95:66:6b:03`) everywhere: JSON,
  CLI, window, metrics. pla-util prints the same NID byte-reversed
  (`036b6695e6f2b0`).
- An unnamed adapter is `Adapter 1B2C`: the last four hex digits of its MAC,
  upper case.
- `rekey` writes remotes first and the local adapter last. `--only MAC`
  limits the write to those adapters and is the only way to write one that is
  not in the config.
- No key material is ever read from an adapter, and no property reads are
  made at all in this version.
- Hooks are exec only, environment prefix `POWERLINE_MONITOR_`; there is no
  built-in webhook.
- Timestamps in JSON are RFC 3339 with the local offset.
- Qualcomm support is deferred. The frame allowlist holds nine messages and
  nothing else.
- Verification is done live against hardware; the repository ships no test
  fixtures or CI.

## Workspace layout

| Crate | Contents |
|---|---|
| `crates/core` | raw socket, frame codec and allowlist, Broadcom and standard MMEs, poll loop, config, history ring, event tracker, text rendering |
| `crates/daemon` | the daemon: HTTP server, metrics, hook runner, state persistence |
| `crates/ui` | the egui window |
| `crates/watch` | the terminal view |
| `crates/ops` | `rekey` and `check-default-key` |
| `src` | the `powerline-monitor` binary: clap definitions and the remaining commands |
