# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **v0.1.0 released** — first Linux release. Download the [latest `.deb`](https://github.com/desmondc9/voice-input-src/releases/latest), or build from source (instructions below).

> **Phase 3 GNOME note**: the overlay uses `wlr-layer-shell`, which GNOME's mutter does NOT implement. `voice-input listen` will fail to position the capsule correctly on GNOME — explicitly out of scope.

## Install

**Recommended:** download the latest `.deb` from the [GitHub Releases page](https://github.com/desmondc9/voice-input-src/releases/latest) and install with `apt`.

```bash
# CPU build (any Linux, no GPU required):
wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input_0.1.0_amd64.deb
sudo apt install ./voice-input_0.1.0_amd64.deb

# NVIDIA GPU users — faster transcription via CUDA:
wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input-cuda_0.1.0_amd64.deb
sudo apt install ./voice-input-cuda_0.1.0_amd64.deb
```

After install, three one-time setup steps:

1. **Download a whisper model** — see [Download a whisper model](#download-a-whisper-model) below.
2. **Install and start `ydotoold`** — see [Install ydotool](#install-ydotool-for-listen-mode-only) below. (Package `ydotool-daemon` is recommended by both `.deb`s so it should already be present; the section explains how to enable the systemd user service.)
3. **Bind a global shortcut on first launch** — the app registers a portal global shortcut. Open your desktop's Global Shortcuts settings (KDE: System Settings → Shortcuts → Global Shortcuts) and assign a key to `voice-input → Hold to dictate`.

Then run `voice-input` to start the tray app.

## Build

Requires Rust 1.83+, `cmake`, `libclang`, and `cc`/`gcc`. On first build, whisper.cpp compiles from source (≈30–60 s).

```bash
cd linux
cargo build --release
```

System packages (Debian/Ubuntu): `sudo apt install cmake clang libclang-dev libasound2-dev`.

### Optional: CUDA acceleration

The default build is CPU-only and requires no special toolkit. For NVIDIA GPU acceleration:

```bash
sudo apt install nvidia-cuda-toolkit
cd linux
cargo build --release --features cuda
```

The CUDA-enabled binary is 5–15× faster on RTX-class GPUs (e.g. 3 s → 200 ms for a 5-second utterance with `large-v3-turbo`). It links against `libcudart` and `libcublas` at runtime.

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

### Default mode (full app)

```bash
RUST_LOG=info cargo run --release
```

A tray icon appears in your system tray (KDE Plasma) or waybar (sway / hyprland — needs the `tray` module). The hotkey + overlay are wired automatically. The menu exposes:

- **Enabled** — master switch. Toggle off to silently ignore the hotkey without quitting.
- **Language ▶** — whisper transcription language (auto-detect / English / 中文 / 日本語 / 한국어 / Español).
- **LLM Refinement ▶** → **Enabled** + **Settings…** — opens the dialog for Base URL / API Key / Model with Test + Save.
- **Quit** — clean shutdown.

All menu changes persist to `~/.config/voice-input/config.toml` automatically; no manual file editing needed.

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

The tray icon switches to a red **record** glyph (`media-record-symbolic`) while audio is being captured, and reverts to the microphone glyph the moment you release the hotkey. The overlay capsule continues to show "Refining…" while the LLM is processing, so you can tell capture-vs-refinement apart at a glance.

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

The `llm_api_base_url` accepts any OpenAI-compatible endpoint (Ollama, vLLM, llama.cpp server, Together, Groq, etc.). The request timeout defaults to 30 s (override via `llm_timeout_secs`). Manual TOML editing is no longer required — use the **LLM Refinement → Settings…** dialog from the tray (Phase 5).

#### Ollama (local) example

For a fully-local setup, run [Ollama](https://ollama.com/) and point voice-input at its OpenAI-compatible endpoint:

~~~bash
ollama pull qwen3.5:2b
ollama serve  # usually already running as a systemd user service
~~~

~~~toml
llm_enabled = true
llm_api_base_url = "http://localhost:11434/v1"
llm_api_key = "ollama"
llm_model = "qwen3.5:2b"
llm_timeout_secs = 30  # optional; default 30. Bump higher for slow cold-starts on bigger models.
~~~

`llm_api_key` can be any non-empty string — Ollama does not validate it, but the refiner short-circuits when the key is empty. Small local models (≤3B) often follow the conservative system prompt loosely; if you see the model rewriting or paraphrasing rather than just fixing ASR errors, try a larger model (7B+) or use a cloud provider for refinement.

### Autostart (Phase 7)

Run once to install a `~/.config/autostart/voice-input.desktop` entry that launches the tray at login:

```bash
./linux/scripts/install-autostart.sh
```

The script picks `voice-input` from your `$PATH`, or falls back to `linux/target/release/voice-input`. Pass an explicit path if you want a different binary:

```bash
./linux/scripts/install-autostart.sh /usr/local/bin/voice-input
```

Remove with:

```bash
rm ~/.config/autostart/voice-input.desktop
```

### Do NOT wrap voice-input in a systemd `--user` service

It looks tempting to run `voice-input` as a `systemctl --user` service so you get `journalctl --user -u voice-input -f` and supervised restarts. **It does not work on KDE Plasma 6.** The global-shortcuts portal will reject the session and you'll see:

```
ERROR voice_input: backend exited with error error=creating portal global-shortcuts session
```

with **nothing** in `journalctl --user -u plasma-xdg-desktop-portal-kde` — the request never makes it to the KDE backend.

Why: KDE's xdg-desktop-portal identifies the calling app by walking the caller's cgroup path looking for a Plasma-launch scope (`/.../app.slice/app-<app_id>-<PID>.scope`). A systemd `.service` unit produces a cgroup like `/.../app.slice/voice-input.service` instead, which carries no recoverable `app_id` for KDE's matcher. `xdg-desktop-portal` then refuses `CreateSession` outright. Renaming the unit to `app-com.yetone.VoiceInput.service`, or launching via `systemd-run --user --scope --unit=app-com.yetone.VoiceInput-XXX`, also fails — KDE expects the exact Plasma launch pattern.

Use the XDG autostart entry above. That's the supported path. KDE launches it inside a proper `app-com.yetone.VoiceInput-<PID>.scope` and the portal sees the correct `app_id`.

### Don't write a custom `ydotoold.service` — use the packaged `ydotool.service`

The `ydotool` Debian package ships `/usr/lib/systemd/user/ydotool.service` (unit name: `ydotool`, **not** `ydotoold`) and enables it by default. It owns `/run/user/$UID/.ydotool_socket`.

If you also create a custom `~/.config/systemd/user/ydotoold.service`, the two units race for the same socket. The custom one will spam:

```
error: Another ydotoold is running with the same socket.
```

restart 100+ times, eventually leaving an **orphan `ydotoold` process** (`PPID = systemd --user`, but not tracked by any unit) that systemd won't clean up. Symptoms: `systemctl --user status ydotoold` says inactive, but `pgrep ydotoold` finds a running process, and `systemctl --user stop ydotool` doesn't help because that's a different unit.

Cleanup:

```bash
# Remove the custom unit (if you created one)
systemctl --user disable --now ydotoold.service
rm -f ~/.config/systemd/user/ydotoold.service ~/.config/systemd/user/default.target.wants/ydotoold.service
systemctl --user daemon-reload

# Kill any orphan ydotoold and stale socket
pkill -9 ydotoold
rm -f /run/user/$UID/.ydotool_socket

# Let the packaged unit take over
systemctl --user restart ydotool.service
systemctl --user is-active ydotool.service   # expect: active
ydotool key 56:1 56:0                         # smoke test — no error means it works
```

### Quick sanity check on first run

```bash
# 1. ydotool daemon is running and reachable
systemctl --user is-active ydotool.service
ydotool key 56:1 56:0

# 2. You're in the `input` group (re-login after install-ydotool.sh)
groups | tr ' ' '\n' | grep -x input

# 3. The whisper model is in place
ls ~/.local/share/voice-input/models/

# 4. KDE Wayland is in use (Plasma 6 portal works only here)
echo "$XDG_SESSION_TYPE $XDG_CURRENT_DESKTOP"   # expect: wayland KDE

# 5. Launch from your desktop session (terminal is fine), NOT from systemd
voice-input &!
```

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

See `../plans/2026-05-15-voice-input-linux/voice-input-linux.md` for the full design and `../plans/2026-05-15-voice-input-linux/implementations/` for per-phase implementation plans (Phase 0 through Phase 7).
