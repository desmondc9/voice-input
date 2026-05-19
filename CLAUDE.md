# CLAUDE.md

This file guides Claude Code working in this repository.

VoiceInput is a Wayland-native hold-to-talk voice input app for Linux. Single Rust crate. The press-and-hold global shortcut is bound through the XDG GlobalShortcuts portal; audio is captured via cpal, sliced by a Silero ONNX VAD detector, transcribed by whisper.cpp (optionally on CUDA), optionally refined through an OpenAI-compatible chat completion, then pasted into the focused application via ydotool. The overlay UI is a GTK4 wlr-layer-shell capsule. Targets KDE Plasma 6, sway, and hyprland; **not supported on GNOME** (mutter lacks wlr-layer-shell).

## Build & Run

```bash
cargo build --release                         # CPU build
cargo build --release --features cuda         # NVIDIA GPU (5–15× faster transcription)

cargo run --release                           # full app: tray + listen + overlay
cargo run --release -- transcribe             # standalone segment-and-print mode
cargo test                                    # unit + integration tests
```

System packages (Debian/Ubuntu):

```
cmake clang libclang-dev libgtk-4-dev libgtk4-layer-shell-dev
libwayland-dev libxkbcommon-dev libasound2-dev pkg-config
```

CUDA build additionally requires `nvidia-cuda-toolkit`.

Runtime requirements:

- `ydotool` installed and the packaged user service running: `systemctl --user is-active ydotool.service` must report `active`.
- A Wayland compositor with `wlr-layer-shell` support (KDE Plasma 6, sway, or hyprland).
- A whisper.cpp model file under `~/.local/share/voice-input/models/` (default `ggml-small.bin`; the project also supports `ggml-large-v3-turbo.bin` for CUDA users).

## Architecture

```
KeyMonitor (XDG portal) ──▶  AppOrchestrator (main.rs)
                                  │
                                  ▼
                          AudioCapture (audio.rs, cpal)
                                  │
                                  ▼
                          VadSlicer (speech/vad.rs, Silero ONNX)
                                  │
                                  ▼
                          WhisperWorker (speech/worker.rs)
                                  │
                                  ▼   (optional)
                          LlmRefiner (refiner.rs)
                                  │
                                  ▼
                          TextInjector (injector.rs, ydotool)
                                  │
                                  ▼
                          focused application

TrayMenu (tray.rs, ksni)      ◀────  status / config
OverlayPanel (overlay/, GTK4) ◀────  waveform + state
```

Per-file responsibilities (`src/`):

- **`main.rs`** — entry point AND backend orchestrator. Sets up logging, builds the tokio runtime, launches the GTK4 main loop on the OS main thread, spawns the async backend (`run_backend_async`) on a worker thread. That backend owns the dictation lifecycle: subscribes to portal hotkey events, starts/stops audio capture, drives whisper transcription, optionally refines via LLM, paste-injects the final text.
- **`lib.rs`** — library crate root. Re-exports the modules used by integration tests under `tests/`.
- **`app.rs`** — defines the `AppState` enum (`Idle`, `Listening`, `Refining`, `Injecting`, `Error`) used as a type-level state machine. Currently a thin module; orchestration lives in `main.rs`.
- **`hotkey.rs`** — XDG portal `GlobalShortcuts` session wrapper. Binds the `toggle_recording` shortcut, exposes `activated`/`deactivated` event streams.
- **`audio.rs`** — cpal input stream management. Opens the default input device, posts raw samples + RMS to channels.
- **`speech/`** — `vad.rs` (Silero ONNX voice-activity slicer; stateful detector reset per dictation) and `worker.rs` (persistent whisper.cpp worker that loads the model once at startup and reuses `WhisperState` between dictations, saving ~1.5–2s startup cost per utterance).
- **`refiner.rs`** — optional OpenAI-compatible chat-completion client. Conservative system prompt: fix obvious ASR errors only, never rewrite.
- **`injector.rs`** — ydotool wrapper. wl-clipboard write + simulated Ctrl+V via the `ydotool` CLI, which talks to the `ydotool.service` daemon over `/run/user/$UID/.ydotool_socket`.
- **`tray.rs`** — ksni-based StatusNotifierItem. Menu: Enabled / Language / LLM Refinement → Settings / Quit. Switches icon between idle (`audio-input-microphone`) and recording (`media-record-symbolic`).
- **`settings_window.rs`** — GTK4 dialog for editing LLM refiner settings (Base URL / API Key / Model) with Test and Save actions. Launched from the tray menu.
- **`overlay/`** — `window.rs` (GTK4 + wlr-layer-shell capsule centered at the bottom of the screen) and `waveform.rs` (5-bar animated waveform driven by audio RMS). Module entrypoint `mod.rs`.
- **`config.rs`** — TOML config at `~/.config/voice-input/config.toml`. Read at startup, written when tray menu options change.
- **`state.rs`** — shared mutable runtime state accessed by tray, overlay, and orchestrator. The persistent `Config` is wrapped in `Arc<parking_lot::Mutex<Config>>`. Also exposes `shutdown: Arc<Notify>` for coordinated Ctrl+C teardown and `recording: Arc<AtomicBool>` driving the tray icon swap between idle and recording states.
- **`cli.rs`** — clap-derived CLI parsing for the subcommand modes (`transcribe`, `listen`, default).
- **`error.rs`** — shared `AppError` / `AppResult` types.

## Conventions When Editing

- **ydotool is the only paste mechanism.** Wayland does not allow synthesizing Ctrl+V from a non-privileged process via `wl-clipboard` alone. Any rewrite of `injector.rs` must preserve the dependency on a running `ydotool.service` (which owns `/dev/uinput`).
- **Do not package voice-input as a systemd `--user` service.** KDE's xdg-desktop-portal identifies the caller through the cgroup scope (`/.../app.slice/app-<app_id>-<PID>.scope`). A systemd `.service` cgroup does not match this pattern, and the portal silently refuses `CreateSession`. The supported launch path is the XDG autostart entry generated by `scripts/install-autostart.sh`. Troubleshooting details live in `README.md`.
- **CUDA arch must be explicit in CI.** whisper.cpp defaults `CMAKE_CUDA_ARCHITECTURES=native`. On a GPU-less CI runner that falls back to nvcc's hardcoded `sm_52`, producing a binary that aborts on any modern card. `.github/workflows/release.yml` sets `CMAKE_CUDA_ARCHITECTURES="75;80;86;89;90"` (Turing → Hopper). Do not remove that env var.
- **Persisted config keys are stable.** `~/.config/voice-input/config.toml` keys (`whisper_model_path`, `whisper_model_size`, `language_hint`, `llm_enabled`, `llm_api_base_url`, `llm_api_key`, `llm_model`, `llm_timeout_secs`) — do not rename without a migration.
- **Tray icons use freedesktop names**, not file paths: `audio-input-microphone` for idle, `media-record-symbolic` for recording.

## Release Process

1. Bump `version = "..."` in `Cargo.toml` on `main`.
2. Add a corresponding entry to `CHANGELOG.md`.
3. Push `main`, then tag and push:
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
4. `.github/workflows/release.yml` builds CPU and CUDA `.deb` variants in parallel (CUDA takes ~30 min for 5 archs) and attaches both to the GitHub Release.
5. Verify both `voice-input_X.Y.Z_amd64.deb` and `voice-input-cuda_X.Y.Z_amd64.deb` are attached at `https://github.com/desmondc9/voice-input/releases/tag/vX.Y.Z`.
