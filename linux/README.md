# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **Phase 0** — scaffold only. No audio or transcription yet. See `../implementation/` for the phased build plan.

## Build

Requires Rust 1.83+ and the GTK4 development packages (used from Phase 3 onward).

```bash
cd linux
cargo build --release
```

## Run

```bash
RUST_LOG=info cargo run
```

A tray icon appears in your system tray (KDE Plasma) or waybar (sway / hyprland — needs the `tray` module).

## Compositor support

- **KDE Plasma 6**: target compositor, built-in StatusNotifierItem host.
- **sway**: requires waybar with `tray` module.
- **hyprland**: requires waybar / ironbar / Riftbar with `tray` module.
- **GNOME**: **not supported.** Mutter lacks `wlr-layer-shell` (needed in Phase 3).

## Config

`~/.config/voice-input/config.toml` — created on first run. Edit and restart to change.

## Project layout

See `../plans/voice-input-linux.md` for the full design and `../implementation/` for per-phase implementation plans.
