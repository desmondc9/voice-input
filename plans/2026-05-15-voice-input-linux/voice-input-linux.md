# Plan: Adding Linux Support to VoiceInput

## Context

`voice-input-src` is a macOS-only menu-bar voice input app built on Apple Speech (`SFSpeechRecognizer`), AppKit (`NSPanel`, `NSStatusItem`, `NSVisualEffectView`), Quartz (`CGEvent` tap on `.maskSecondaryFn`), and Carbon (`TIS*` input source APIs). Every subsystem hits an Apple-proprietary framework — there is essentially nothing to share with Linux. Adding Linux support is therefore a **parallel rewrite**, not a refactor.

Goal: ship a Linux-native binary that reproduces the macOS feature set (hold-to-record hotkey → streaming transcription → optional conservative LLM refinement → paste into focused app → animated capsule overlay with live RMS waveform) on **Wayland (KDE Plasma 6, sway, hyprland)** using **Rust** and **local whisper.cpp**. GNOME is explicitly out of scope (mutter lacks `wlr-layer-shell`).

This plan was refined through a structured brainstorm (see "Brainstorm revisions" at the end of this document). Major UX/architecture decisions are locked in below.

## Strategic decisions

| Axis | Choice | Why |
|---|---|---|
| Speech engine | whisper.cpp via `whisper-rs` + hand-written VAD slicing | Offline, private, multilingual; avoid single-maintainer `whisper-cpp-plus` 0.1.x dependency |
| Streaming UX | VAD-sliced segments (append, do not overwrite) | Whisper partials churn — appending stable segments after speech pauses avoids the rewriting jitter |
| Display server | Wayland-first (Plasma 6 / sway / hyprland) | Target compositors |
| Visual design | New Linux-native capsule aesthetic — do NOT mimic macOS blur | GTK4 + layer-shell can't reproduce `NSVisualEffectView .hudWindow` reliably; pretending otherwise looks worse |
| Keystroke injection | **ydotool hard dependency** (uinput) | Only universally reliable Wayland path; KDE Plasma 6 rejects `virtual-keyboard`, portal RemoteDesktop UX is rough |
| Project identity | Linux version is a normal design-doc-driven project, NOT a `claude -p` one-shot reproducer | Stack complexity exceeds reliable single-prompt regeneration; be honest about this |
| Language | Rust | Mature crates for every subsystem |
| Scope | Full feature parity with macOS (capsule overlay, RMS waveform, LLM refiner, language menu) | Per user decision |
| GNOME | Out of scope | Mutter lacks `wlr-layer-shell`; detect and surface clear error |

## Repository layout

```
voice-input-src/
├── README.md, README_CN.md   # existing macOS prompts (unchanged)
├── dist/                     # existing macOS submodule (unchanged)
└── linux/                    # NEW: Linux Rust project
    ├── Cargo.toml
    ├── README.md             # Build instructions, ydotool setup, compositor matrix
    ├── scripts/
    │   └── install-ydotool.sh  # one-shot udev rule + systemd user service
    ├── packaging/
    │   └── AppImage/
    └── src/
        ├── main.rs           # entry: tokio runtime on worker thread, GTK on main thread
        ├── app.rs            # AppState enum + transitions (orchestrator)
        ├── hotkey.rs         # ashpd GlobalShortcuts press/release
        ├── audio.rs          # cpal capture + RMS normalization
        ├── speech/
        │   ├── mod.rs        # whisper-rs context, worker thread
        │   └── vad.rs        # voice activity detector + segment slicing
        ├── overlay/
        │   ├── mod.rs        # GTK4 + gtk4-layer-shell capsule (Linux-native styling)
        │   └── waveform.rs   # 5-bar DrawingArea — keep weights/smoothing from macOS
        ├── injector.rs       # ydotool shell-out + clipboard via wl-clipboard-rs
        ├── refiner.rs        # OpenAI-compatible HTTP refiner (direct port)
        ├── settings.rs       # GTK Settings window
        ├── tray.rs           # ksni status notifier
        ├── config.rs         # ~/.config/voice-input/config.toml
        ├── model_download.rs # first-run wizard with resume support
        └── error.rs          # AppError + ErrorKind enum
```

No `dist-linux/` submodule (reproducibility contract is dropped; AppImage ships via GitHub Releases).

## Application state machine

Explicit, exhaustive — error paths are not optional.

```rust
enum AppState {
    Idle,
    Listening { started_at: Instant },
    Refining { raw_text: String },
    Injecting { final_text: String },
    Error(ErrorKind),
}

enum ErrorKind {
    NoMicrophone,        // cpal device error
    ModelMissing,        // whisper model file not found
    WhisperFailed(String),
    PortalRevoked,       // user dismissed/revoked GlobalShortcuts session
    YdotoolMissing,      // ydotool binary or daemon unavailable
    NetworkError(String), // refiner HTTP failure
}
```

Every transition handles errors. Entering `Error(_)` dismisses the overlay, surfaces a tray notification with the kind, then drops back to `Idle`.

## Module port map

| macOS source | Linux module | Crate(s) | Notes |
|---|---|---|---|
| `main.swift` | `main.rs` | `tokio`, `gtk4` | GTK on the main thread; one tokio runtime on a background thread; bridge via `glib::MainContext::channel` |
| `AppDelegate.swift` | `app.rs` | `tokio::sync::mpsc`, `tokio::sync::watch` | `AppState` machine above. Each transition is an explicit method; errors are explicit branches. |
| `KeyMonitor.swift` (CGEvent Fn tap) | `hotkey.rs` | `ashpd` (portal `GlobalShortcuts`) | `receive_activated()` / `receive_deactivated()` for press/release. Fn is unavailable on Linux — first run shows portal dialog; recommended chord: Right Ctrl. Persist `shortcut_handle` so subsequent runs are silent. Document a compositor-binding fallback (sway `bindsym --no-repeat`, hyprland `bindd`/`bindr`) that pokes a unix socket. |
| `SpeechEngine.swift` (SFSpeechRecognizer + AVAudioEngine + RMS) | `audio.rs` + `speech/mod.rs` + `speech/vad.rs` | `cpal`, `whisper-rs`, `voice_activity_detector` (or `webrtc-vad`) | Audio capture: one cpal input stream → tap fan-outs to (a) RMS computation, (b) whisper feed buffer. VAD slices on speech pauses (~300 ms silence); each slice is a full `whisper_full()` call producing one stable text segment. Segments append to overlay text. **Language** is a hint passed via `whisper_full_params.set_language(Some("zh"))` — single multilingual model, not a per-language model swap. |
| `OverlayPanel.swift` + `WaveformView` | `overlay/mod.rs` + `overlay/waveform.rs` | `gtk4`, `gtk4-layer-shell`, `cairo-rs` | Layer-shell with `layer = Overlay`, `anchor = Bottom`, `margin = 56`, `keyboard_interactivity = None`. **Linux-native visual** (see Visual design below) — no blur attempt. Waveform: keep the `[0.5, 0.8, 1.0, 0.75, 0.55]` weights, attack 0.4 / release 0.15 smoothing, ±4% jitter — those constants are good visual design and worth preserving 1:1. Text: appendable segments rather than rewritten label. |
| `LLMRefiner.swift` | `refiner.rs` | `reqwest`, `serde_json`, `tokio` | Pure HTTP — direct port. Keep the conservative system prompt **verbatim** (it's part of the product contract). `force` flag for Settings → Test. |
| `TextInjector.swift` | `injector.rs` | `wl-clipboard-rs`, `tokio::process` | 1) snapshot clipboard via `wl-clipboard-rs` → 2) write transcription → 3) `tokio::process::Command::new("ydotool").args(["key","ctrl+v"])` → 4) restore original clipboard after ~500 ms. **No IME swap** — fcitx5/ibus don't intercept Ctrl+V on Linux. **No virtual-keyboard, no portal RemoteDesktop** — ydotool is the only path. |
| `SettingsWindow.swift` | `settings.rs` | `gtk4` | Three `Entry` widgets (Base URL / API Key / Model) + Test + Save buttons. Also shows: active speech backend (CPU / CUDA / Vulkan), model path, ydotoold status. |
| `setupStatusBar()` | `tray.rs` | `ksni` | StatusNotifierItem with Enabled / Language submenu / LLM Refinement (Enable + Settings…) / Quit. |
| `UserDefaults` keys | `config.rs` | `serde`, `toml`, `directories` | TOML at `~/.config/voice-input/config.toml`. Keys: `language_hint`, `llm_enabled`, `llm_api_base_url`, `llm_api_key`, `llm_model`, `shortcut_handle`, `whisper_model_path`, `whisper_model_size`. |
| (new) First-run model download | `model_download.rs` | `reqwest`, `gtk4` | GTK wizard: pick model size (tiny 75 MB / base 142 MB / **small 466 MB default** / medium 1.5 GB), shows progress bar, supports HTTP `Content-Range` resume on retry. Offline detection → friendly error + manual path option. Models pulled from `huggingface.co/ggerganov/whisper.cpp`. |
| Logging | (cross-cutting) | `tracing`, `tracing-subscriber` | `~/.local/state/voice-input/voice-input.log` (XDG state dir). |

## Visual design (Linux-native capsule)

Do not mimic NSVisualEffectView. Design choices:

- Background: `oklch(20% 0.01 280 / 0.92)` — dark, slightly desaturated, 92% alpha. Solid color, not blur. Acceptable on every Wayland compositor.
- Border: 0.5 px inner border at `rgba(255, 255, 255, 0.10)` for definition.
- Shadow: GTK4 CSS `box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45)` — readable on light or dark wallpapers.
- Corner radius: 28 px (matching macOS — geometry is fine to copy, blur is not).
- Optional: subtle CSS `linear-gradient` overlay to add visual interest where macOS used vibrancy. Tunable in design phase.
- Waveform bar colors: `rgba(255, 255, 255, 0.92)` — same as macOS.

Keep all animation timings from `OverlayPanel.swift`: 0.35 s entry spring, 0.25 s width transition, 0.22 s exit. Those values are good interaction design and translate directly to GTK adjustments + `glib::timeout_add_local`.

## Async / threading model

Single tokio current-thread runtime on a dedicated background thread. GTK4 lives on the main thread.

- **Main thread**: GTK4 event loop. UI mutations only.
- **Tokio worker thread**: HTTP (refiner), audio capture (cpal callback runs on cpal's own thread but messages route through tokio channels), portal D-Bus.
- **Whisper worker thread**: a dedicated `std::thread` because `whisper_rs::WhisperState` is `!Send`. Receives audio slices via `crossbeam_channel`, emits text segments back via the same channel type.
- **Bridge**: `glib::MainContext::channel::<UiEvent>(glib::PRIORITY_DEFAULT)` — workers push `UiEvent` variants (`SegmentAppended(String)`, `AudioLevel(f32)`, `StateChanged(AppState)`, `Error(ErrorKind)`), UI thread receives and updates widgets.

## GPU acceleration

Compile-time feature flags on `whisper-rs`:

- `--features cuda` (NVIDIA)
- `--features vulkan` (cross-vendor; recommended GPU default once it stabilizes)
- `--features hipblas` (AMD ROCm)
- Default release: CPU. README documents how to enable GPU; Settings UI surfaces which backend is active at runtime.

## Testing strategy

To satisfy the global 80% coverage rule:

**Unit tests** (`cargo test`):
- `audio::rms_normalize` — dB → 0..1 mapping, edge cases (silence, clipping)
- `speech::vad::slice` — given a synthetic audio stream with known silences, produce expected slice boundaries
- `config::roundtrip` — write defaults → read → compare
- `refiner::parse_response` — given OpenAI-shaped JSON, extract content; given error JSON, surface error
- `app::transitions` — drive `AppState` machine through expected and error paths, assert side effects

**Integration tests** (`tests/`):
- `transcribe_wav.rs` — feed `tests/fixtures/zh_python.wav` through `audio + vad + whisper` (using a small CPU model checked into LFS or downloaded on CI), assert transcript contains "Python"
- `refiner_mock.rs` — `wiremock` for OpenAI API; verify retry + auth header + body

**Manual end-to-end** (compositor matrix):
- Plasma 6 / sway / hyprland: full UX walkthrough (see Verification below)

**Skip**: GTK widget tests (`gtk4-test` is awkward and brittle); visual regression of the overlay (manual screenshots suffice given the bespoke design).

## Wayland gotchas & mitigations

1. **Portal GlobalShortcuts requires interactive binding on first run.** Onboarding screen explains it. Sway/hyprland users can use compositor-native binding into a unix socket as a fallback.
2. **virtual-keyboard protocol doesn't work on KDE Plasma 6.** Not used — ydotool is the only injection path.
3. **Layer-shell isn't on GNOME mutter.** Detect mutter at startup; show clear error and exit cleanly.
4. **Clipboard contents die with the offering client.** Process stays alive through the entire paste sequence (sequential async). 500 ms delay before restoring original contents.
5. **Tray hosting on sway/hyprland needs an SNI host.** waybar (or ironbar/Riftbar) with `tray` module. Plasma 6 has built-in host. README documents this.
6. **Whisper VAD-sliced segments still take time per slice.** Expect 200–600 ms per segment on `whisper-small` CPU. The overlay shows the segment appearing after each speech pause, which is honest UX — no fake real-time pretense.
7. **Microphone permissions.** Linux doesn't gate this like macOS. No explicit permission code; if cpal fails to open the default device, surface `NoMicrophone` error.

## Hotkey default

Fn is firmware-handled on most Linux laptops and invisible to userspace. Replace with a **user-chosen chord via the portal dialog**. Suggested guidance in the binding dialog: **Right Ctrl** (single key, easy to hold, rarely bound). Persist the portal handle.

## Build & distribution

- Toolchain: stable Rust 1.83+, `cmake`, `gcc`/`clang`, `libgtk-4-dev`, `libgtk4-layer-shell-dev`, `pkg-config`, `libpipewire-0.3-dev`.
- Runtime: `ydotool` + `ydotoold` running as user service (install script provided).
- Build: `cargo build --release` → `linux/target/release/voice-input`.
- Whisper model: GTK wizard on first run, default `ggml-small.bin` to `~/.local/share/voice-input/models/`.
- Packaging: **AppImage** (linuxdeploy-plugin-gtk) published to GitHub Releases. Flatpak deferred.
- Autostart: `~/.config/autostart/voice-input.desktop` installed from Settings UI.

## Phased build sequence

Adjusted to 3–4 weeks of focused work:

1. **Phase 0 — scaffold** (1–2 days): Cargo workspace, GTK4 hello-world window, `ksni` tray with Quit, `config.rs` round-trip, `error.rs` skeleton. Verify toolchain on Plasma 6 + sway + hyprland.
2. **Phase 1 — audio + VAD + speech** (4–5 days): `cpal` capture + RMS. `voice_activity_detector` integration. `whisper-rs` worker thread with sliding-window slicing on VAD boundaries. CLI: `cargo run -- transcribe-stdin` reads mic, prints segments. No UI.
3. **Phase 2 — hotkey + paste loop** (2–3 days): `ashpd` GlobalShortcuts press/release. `wl-clipboard-rs` snapshot/restore. `ydotool` shell-out. End-to-end: hold key, speak, release, transcript pastes into another window. Still CLI-only.
4. **Phase 3 — overlay + waveform** (4–5 days): `gtk4-layer-shell` capsule with Linux-native styling. Custom-drawn 5-bar waveform with macOS-derived constants. Width animation. Refining state. Appendable segment rendering.
5. **Phase 4 — LLM refiner** (1 day): direct port of `LLMRefiner.swift`; system prompt verbatim.
6. **Phase 5 — Settings + tray menus + state machine wiring** (2–3 days): GTK Settings dialog, language hint menu, Enable toggle, LLM Refinement submenu. Wire `AppState` transitions across all modules.
7. **Phase 6 — first-run wizard + error UX** (2 days): Model download wizard with resume. Onboarding for portal hotkey binding. Tray notifications for each `ErrorKind`.
8. **Phase 7 — testing + CI** (2–3 days): unit + integration tests to 80%+. GitHub Actions runs `cargo test` (audio integration uses pre-recorded fixtures).
9. **Phase 8 — packaging** (2 days): AppImage build, README with compositor matrix + ydotool setup + troubleshooting, GitHub Releases workflow.

**Total: ~3–4 weeks.**

## Critical files referenced from the macOS source

Port behavior 1:1 from these — they encode product decisions worth preserving:
- `dist/Sources/VoiceInput/LLMRefiner.swift:46-63` — system prompt (copy verbatim)
- `dist/Sources/VoiceInput/SpeechEngine.swift:97-103` — RMS → 0–1 normalization (`(dB + 50) / 40`, clamp)
- `dist/Sources/VoiceInput/OverlayPanel.swift:181-217` — waveform weights, attack/release, jitter
- `dist/Sources/VoiceInput/OverlayPanel.swift:97-135` — entry/width/exit animation timings (0.35 / 0.25 / 0.22)
- `dist/Sources/VoiceInput/AppDelegate.swift:111-171` — finish-transcription flow (refining-then-inject sequencing); adapt to `AppState` machine

## Verification (end-to-end)

Test on **KDE Plasma 6**, **sway**, **hyprland**:

1. Launch binary → tray icon appears (Plasma) or appears in waybar (sway/hyprland).
2. First run: portal prompts to bind a global shortcut → bind Right Ctrl. Model download wizard appears → download `small`.
3. Open a text editor (Kate, gedit, alacritty + nvim). Focus the input field.
4. Hold Right Ctrl → capsule fades in centered at bottom, waveform reacts to speech RMS in real time.
5. Speak `"Python 和 JSON"` — wait for a natural pause — first segment appears in capsule. Continue speaking; subsequent segments append.
6. Release Right Ctrl → if LLM refiner enabled, "Refining…" state shows; otherwise paste happens immediately via ydotool.
7. Confirm: text lands in the focused editor; original clipboard contents (set a known string beforehand) restore after ~500 ms.
8. Change language hint to 日本語 in the tray submenu → repeat with Japanese phrase.
9. Open Settings → enter API base URL / API key / model → Test → see "OK: …".
10. Toggle "Enabled" off → hotkey no longer captures.
11. Force error paths: unplug mic mid-recording → `NoMicrophone` error tray notification, overlay dismissed cleanly. Kill `ydotoold` → next paste shows `YdotoolMissing` error.
12. Quit from tray → process exits cleanly, no orphan GTK windows.

Non-functional checks:
- `whisper-small` per-segment latency under 600 ms on the target machine (post-VAD).
- Capsule animation stays at 60 fps (use `GTK_DEBUG=interactive`).
- Memory after 50 record-cycles flat (no whisper context leak).
- Test coverage ≥ 80% via `cargo tarpaulin`.

## Open risks / non-goals

- **GNOME not supported.** Documented; startup-time detection.
- **ydotool prerequisite is a real onboarding cost.** Mitigated with `install-ydotool.sh`; not eliminated.
- **Portal GlobalShortcuts on sway** is the least-mature backend. Compositor-binding fallback documented.
- **VAD-sliced UX is honest but different from macOS streaming.** Users should not expect identical feel — segments appear after natural pauses, not character-by-character.
- **No code sharing with macOS Swift.** Only the LLM system prompt and the product spec are shared (and a handful of magic constants for waveform/animation).
- **Linux version does NOT follow the macOS reproducibility-prompt contract.** Documented departure.

## Brainstorm revisions (from review session)

This plan was critiqued and refined through a brainstorm session. Decisions made during that review:

1. **Streaming UX**: chose VAD-sliced appended segments over (a) only-final, (b) tiny+small dual model. Rationale: most authentic to user mental model ("I paused, so a segment finalized") without doubling compute.
2. **Visual fidelity**: chose Linux-native capsule design over mimicking macOS blur. Rationale: GTK4 + layer-shell can't reliably reproduce `NSVisualEffectView`; honest divergence beats degraded imitation.
3. **Keystroke injection**: chose ydotool hard dependency over portal+fallback chain. Rationale: only universally reliable Wayland path; simpler code; KDE Plasma 6 rejects `virtual-keyboard`.
4. **Whisper crate**: chose `whisper-rs` + hand-written VAD over `whisper-cpp-plus`. Rationale: `whisper-cpp-plus` is single-maintainer 0.1.x; ~150 lines of VAD slicing is manageable and removes a fragile dep.
5. **Project identity**: dropped the `claude -p` reproducibility contract for the Linux version. Rationale: Rust + GTK4 + whisper.cpp stack complexity exceeds reliable single-prompt regeneration.

Additional refinements added without further user input (recommendations):
- Explicit `AppState` + `ErrorKind` state machine
- First-run model download wizard with resume
- GPU acceleration via compile-time feature flags
- Explicit unit + integration test plan to satisfy 80% coverage rule
- Single tokio runtime + `glib::MainContext::channel` bridge to GTK
- Language menu semantics corrected: it's a whisper *language hint*, not a recognizer swap
- Timeline revised from ~2 weeks to ~3–4 weeks
