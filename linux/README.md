# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **Phase 4** — optional LLM refinement of transcripts before paste (OpenAI-compatible APIs). Overlay (Phase 3), tray, transcribe CLI, and listen mode all still work.

> **Phase 3 GNOME note**: the overlay uses `wlr-layer-shell`, which GNOME's mutter does NOT implement. `voice-input listen` will fail to position the capsule correctly on GNOME — explicitly out of scope.

## Build

Requires Rust 1.83+, `cmake`, `libclang`, and `cc`/`gcc`. On first build, whisper.cpp compiles from source (≈30–60 s).

```bash
cd linux
cargo build --release
```

System packages (Debian/Ubuntu): `sudo apt install cmake clang libclang-dev libasound2-dev`.

## Download a whisper model

```bash
mkdir -p ~/.local/share/voice-input/models
curl -L -o ~/.local/share/voice-input/models/ggml-small.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

Sizes: `tiny` (75 MB), `base` (142 MB), `small` (466 MB, default), `medium` (1.5 GB). Override the path with `VOICE_INPUT_MODEL_PATH=/path/to/model.bin` or by setting `whisper_model_path` in `~/.config/voice-input/config.toml`.

## Install ydotool (for `listen` mode only)

The listen mode pastes text by invoking `ydotool` to simulate Ctrl+V. This requires the `ydotoold` daemon to be running with `/dev/uinput` access. Bootstrap with the included script:

```bash
./scripts/install-ydotool.sh
```

The script installs the `ydotool` package, adds a udev rule, joins you to the `input` group, and enables a systemd user service for `ydotoold`. **Log out and log back in afterward** for group membership to take effect.

## Run

### Tray mode (Phase 0 behavior)

```bash
RUST_LOG=info cargo run
```

A tray icon appears in your system tray (KDE Plasma) or waybar (sway / hyprland — needs the `tray` module).

### Transcribe mode (Phase 1)

```bash
RUST_LOG=info cargo run --release -- transcribe
```

> Use `--release`. Debug-profile whisper inference is 5–10× slower; you'll get poor results without it.

Reads from the default microphone, slices speech on natural pauses (≥300 ms silence), and prints each transcribed segment. Press Ctrl+C to stop.

Example output:
```text
[segment 1] 你好世界
[segment 2] this is a test
```

### Listen mode (Phase 2)

```bash
RUST_LOG=info cargo run --release -- listen
```

First run: the XDG portal prompts you to bind a global shortcut. Recommended: **Right Ctrl** (single key, easy to hold, rarely conflicts).

Then: focus any text input, hold the configured key, speak, release. The transcript is pasted into the focused field. Press Ctrl+C in the terminal to stop the daemon.

Requires the steps in "Install ydotool" above.

### Overlay capsule (Phase 3)

When you hold the configured shortcut, a small dark capsule appears at the bottom-center of your screen with an animated 5-bar waveform that tracks your speaking volume. When you release, the capsule briefly shows "Refining…" then disappears as the text is pasted.

Requires `wlr-layer-shell` support in your compositor:
- **KDE Plasma 6**: works (KWin 6+ supports it).
- **sway / hyprland**: works.
- **GNOME**: not supported — mutter does not implement layer-shell.

### LLM refinement (Phase 4)

Optionally pass the raw transcript through an OpenAI-compatible chat completion before pasting. The system prompt is intentionally conservative — it fixes ASR errors (`配森 → Python`, `杰森 → JSON`, etc.) but does NOT rewrite or polish the text. If the API is unreachable, paste falls back to the raw transcript.

Configure via `~/.config/voice-input/config.toml`:

~~~toml
llm_enabled = true
llm_api_base_url = "https://api.openai.com/v1"
llm_api_key = "sk-..."
llm_model = "gpt-4o-mini"
~~~

The `llm_api_base_url` accepts any OpenAI-compatible endpoint (vLLM, llama.cpp server, Together, Groq, etc.). The 10 s request timeout matches the macOS app. A future Settings UI (Phase 5) will replace manual TOML editing.

## Compositor support

- **KDE Plasma 6**: target compositor, built-in StatusNotifierItem host. Portal `GlobalShortcuts` works out of the box.
- **sway**: requires waybar with `tray` module. Portal support via `xdg-desktop-portal-wlr` (less mature than KDE; document a compositor-binding fallback if portal proves unreliable).
- **hyprland**: requires waybar / ironbar / Riftbar with `tray` module. Portal support via `xdg-desktop-portal-hyprland`.
- **GNOME**: **not supported.** Mutter lacks `wlr-layer-shell` (needed in Phase 3).

## Config

`~/.config/voice-input/config.toml` — created on first run. Edit and restart to change. Notable keys:

- `language_hint = "zh"` — passed to whisper as a hint (`"en"`, `"ja"`, etc., or empty for auto-detect)
- `whisper_model_size = "small"` — determines the default model path

## Project layout

See `../plans/voice-input-linux.md` for the full design and `../implementation/` for per-phase implementation plans.
