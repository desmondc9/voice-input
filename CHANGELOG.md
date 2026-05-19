# Changelog

All notable changes to the Linux build of voice-input are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0: API, CLI surface, and config schema may change without notice between
minor versions.

## [0.1.1] - 2026-05-19

### Fixed

- **CUDA `.deb` crashed on every modern NVIDIA GPU.** The CI release build
  left `CMAKE_CUDA_ARCHITECTURES` unset, so whisper.cpp's default of `native`
  fell back to `sm_52` (Maxwell, GTX 9xx era) on the GPU-less runner.
  Inference aborted on Turing+ cards with
  `CUDA kernel mul_mat_vec has no device code compatible with CUDA arch N`,
  followed by `cublasGemmStridedBatchedEx` failing. The CUDA `.deb` now
  ships fat device code for Turing (75), Ampere (80, 86), Ada (89), and
  Hopper (90), covering T4, RTX 20/30/40 series, A100, and H100.
- Local CPU builds were unaffected; local CUDA builds were unaffected because
  `native` correctly detects the local GPU.

## [0.1.0] - 2026-05-18

First publishable Linux release. All Phase 0–7 features present.

### Added

- **Audio capture & VAD pipeline** (Phase 1) — `cpal` input + Silero ONNX
  voice activity detector + whisper.cpp transcription, segmented at speech
  boundaries.
- **Hotkey + paste pipeline** (Phase 2) — XDG Portal global shortcut binding;
  `wl-clipboard` write + `ydotool` Ctrl+V emulation for Wayland.
- **GTK4 overlay** (Phase 3) — `wlr-layer-shell` capsule with live waveform,
  centered at the bottom of the focused output.
- **LLM refiner** (Phase 4) — optional pass through any OpenAI-compatible
  endpoint (Ollama, OpenAI, etc.) for transcript polishing.
- **Settings tray menu** (Phase 5) — `ksni` tray with submenus for enabled
  state, language, model size, refiner settings; persisted to
  `~/.config/voice-input/config.toml`.
- **Recording-state tray icon** (Phase 6) — animated tray icon that pulses
  during active dictation.
- **Latency polish + autostart** (Phase 7) — persistent whisper / VAD worker
  threads (eliminates per-dictation model load), VAD silence cutoff tuned to
  150 ms, XDG autostart installer script.
- **CUDA acceleration** — opt-in via `cargo build --release --features cuda`
  or by installing the `voice-input-cuda` `.deb`. 5–15× speedup on RTX-class
  GPUs.
- **Packaging** — `voice-input` (CPU) and `voice-input-cuda` (GPU) `.deb`
  packages built and published by a tag-triggered GitHub Actions workflow.

### Known limitations

- Wayland only — explicitly does not target X11.
- GNOME's mutter does not implement `wlr-layer-shell`, so the overlay is
  mis-positioned on GNOME. KDE Plasma, sway, hyprland are supported.
- `amd64` builds only. `arm64` deferred to a future release.
- Whisper models, the `ydotoold` daemon, and the portal global-shortcut
  binding are user-installed steps, not bundled in the `.deb`.

[0.1.0]: https://github.com/desmondc9/voice-input/releases/tag/v0.1.0
