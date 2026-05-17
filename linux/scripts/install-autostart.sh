#!/usr/bin/env bash
# Installs an XDG autostart entry so `voice-input` runs at login.
# Usage: ./install-autostart.sh [/path/to/voice-input]
# If no path is given, looks for `voice-input` in $PATH first, then
# falls back to the repo's release build.

set -euo pipefail

bin_path="${1:-}"

if [[ -z "$bin_path" ]]; then
    if command -v voice-input >/dev/null 2>&1; then
        bin_path="$(command -v voice-input)"
    else
        # Resolve repo-relative path from this script's location.
        script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
        candidate="$script_dir/../target/release/voice-input"
        if [[ -x "$candidate" ]]; then
            bin_path="$(realpath "$candidate")"
        else
            echo "error: voice-input not in PATH and no release build at $candidate" >&2
            echo "       pass an explicit path: $0 /full/path/to/voice-input" >&2
            exit 1
        fi
    fi
fi

if [[ ! -x "$bin_path" ]]; then
    echo "error: $bin_path is not executable" >&2
    exit 1
fi

# Resolve to absolute path
bin_path="$(cd "$(dirname "$bin_path")" && pwd)/$(basename "$bin_path")"

autostart_dir="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
mkdir -p "$autostart_dir"
target="$autostart_dir/voice-input.desktop"

cat >"$target" <<EOF
[Desktop Entry]
Type=Application
Name=VoiceInput
Comment=Wayland-native hold-to-talk voice input (tray + overlay + LLM refine)
Exec=$bin_path
Terminal=false
Categories=Utility;AudioVideo;
StartupNotify=false
X-GNOME-Autostart-enabled=true
EOF

echo "installed: $target"
echo "binary:    $bin_path"
echo ""
echo "Log out and back in to test, or run \`$bin_path\` directly to verify."
