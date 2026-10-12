#!/usr/bin/env bash
# User-level install of powerline-monitor: ~/.local/bin binary with CAP_NET_RAW, desktop entry, icons,
# shell completions, config; --service also installs and starts a systemd user unit for the daemon.
set -euo pipefail
cd "$(dirname "$0")"

service=0
for a in "$@"; do
  case "$a" in
    --service) service=1 ;;
    *) echo "usage: $0 [--service]" >&2; exit 2 ;;
  esac
done

bin="$HOME/.local/bin/powerline-monitor"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor"
cfg="${XDG_CONFIG_HOME:-$HOME/.config}/powerline-monitor/config.toml"
unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/powerline-monitor.service"
state="$HOME/.local/state/powerline-monitor"

echo "==> cargo build --release"
cargo build --release

echo "==> install $bin"
install -Dm755 target/release/powerline-monitor "$bin"

echo "==> sudo setcap cap_net_raw+ep $bin"
sudo setcap cap_net_raw+ep "$bin"

echo "==> install $apps/powerline-monitor.desktop"
install -Dm644 packaging/powerline-monitor.desktop "$apps/powerline-monitor.desktop"
sed -i "s|^Exec=.*|Exec=$bin ui|" "$apps/powerline-monitor.desktop"

echo "==> install icons under $icons"
for s in 16 22 32 48 64 128; do
  install -Dm644 "packaging/icons/powerline-monitor-$s.png" "$icons/${s}x${s}/apps/powerline-monitor.png"
done
install -Dm644 packaging/powerline-monitor.svg "$icons/scalable/apps/powerline-monitor.svg"
install -Dm644 packaging/powerline-monitor-symbolic.svg "$icons/symbolic/apps/powerline-monitor-symbolic.svg"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  echo "==> gtk-update-icon-cache $icons"
  gtk-update-icon-cache -q -t -f "$icons"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  echo "==> update-desktop-database $apps"
  update-desktop-database "$apps"
fi

echo "==> shell completions"
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions" \
  "$HOME/.local/share/bash-completion/completions" \
  "$HOME/.local/share/zsh/site-functions"
"$bin" completions fish > "${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions/powerline-monitor.fish"
"$bin" completions bash > "$HOME/.local/share/bash-completion/completions/powerline-monitor"
"$bin" completions zsh > "$HOME/.local/share/zsh/site-functions/_powerline-monitor"
echo "    zsh: add 'fpath=(~/.local/share/zsh/site-functions \$fpath)' before compinit in ~/.zshrc"

if [ -e "$cfg" ]; then
  echo "==> keeping existing $cfg"
else
  list=""
  for d in /sys/class/net/*; do
    [ "$(cat "$d/type" 2>/dev/null)" = 1 ] || continue
    [ "$(cat "$d/operstate" 2>/dev/null)" = up ] || continue
    list="$list\"$(basename "$d")\", "
  done
  list="${list%, }"
  echo "==> create $cfg with interfaces = [$list]"
  install -Dm644 packaging/powerline-monitor.toml.example "$cfg"
  sed -i "s|^interfaces *=.*|interfaces = [$list]|" "$cfg"
fi

if [ "$service" = 1 ]; then
  echo "==> install $unit"
  mkdir -pm700 "$state"
  mkdir -p "$(dirname "$unit")"
  cat > "$unit" <<UNIT
[Unit]
Description=Powerline Monitor: HomePlug AV poller and Prometheus exporter

[Service]
ExecStart=$bin daemon
Environment=STATE_DIRECTORY=%h/.local/state/powerline-monitor
Environment=POWERLINE_MONITOR_HOST=%H
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
UNIT
  echo "==> systemctl --user enable --now powerline-monitor"
  systemctl --user daemon-reload
  systemctl --user enable --now powerline-monitor
  echo "    check: systemctl --user status powerline-monitor; journalctl --user -u powerline-monitor -f; curl -s http://127.0.0.1:9702/api/status"
fi

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "warning: $HOME/.local/bin is not on PATH; add it or run $bin directly" >&2 ;;
esac

echo "done: run 'powerline-monitor status' or launch Powerline Monitor from the application menu"
