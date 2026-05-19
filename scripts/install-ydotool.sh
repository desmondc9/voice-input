#!/bin/bash
# Bootstrap ydotool for Phase 2 listen mode.
#
# Installs the ydotool binary, sets up the /dev/uinput permissions via udev,
# adds the current user to the `input` group, and enables a systemd user
# service for the ydotoold daemon.
#
# Run once after installing the voice-input binary. Requires sudo for the
# apt install + udev rule + group add steps. The systemd user service is
# installed without sudo into ~/.config/systemd/user/.
#
# After this script, log out and log back in for the input-group membership
# to take effect, then verify with:
#   ydotool key 28:1 28:0
# (which simulates the Enter key — useful for a no-op test).

set -euo pipefail

if ! command -v sudo >/dev/null; then
  echo "Error: sudo not found; this script needs sudo to install packages and set udev rules."
  exit 1
fi

echo ">>> Installing ydotool via apt..."
sudo apt-get update
sudo apt-get install -y ydotool

echo ">>> Installing udev rule for /dev/uinput..."
sudo tee /etc/udev/rules.d/80-uinput.rules > /dev/null <<'EOF'
KERNEL=="uinput", GROUP="input", MODE="0660"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger

echo ">>> Adding $USER to the 'input' group..."
sudo usermod -aG input "$USER"

echo ">>> Installing systemd --user unit for ydotoold..."
mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/ydotoold.service" <<'EOF'
[Unit]
Description=ydotool daemon
After=default.target

[Service]
Type=simple
ExecStart=/usr/bin/ydotoold
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now ydotoold

echo
echo "============================================================"
echo "ydotool installed."
echo
echo "IMPORTANT: log out and log back in for input-group membership"
echo "to take effect. Verify with:"
echo
echo "  groups | grep input"
echo "  systemctl --user status ydotoold"
echo "  ydotool key 28:1 28:0   # simulates the Enter key"
echo "============================================================"
