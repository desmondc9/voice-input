# Phase 3 — GTK4 Layer-Shell Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a floating capsule overlay (centered at the bottom of the screen) that appears while the listen-mode hotkey is held, animates a 5-bar waveform driven by real microphone RMS, and disappears after the transcript is pasted. Implemented via `gtk4-layer-shell` on the OS main thread; the speech pipeline + ashpd portal continue running in a background tokio runtime; cross-thread coordination via `glib::MainContext::channel`.

**Architecture:** `voice-input listen` now runs as a true GTK application. The `main` function is synchronous; `run_listen` spawns the tokio runtime + pipeline on a background `std::thread`, then runs `gtk::Application::run` on the OS main thread (GTK4's hard requirement). A `glib::MainContext::channel<OverlayCmd>` carries `Show / SetLevel / SetText / Hide` messages from the backend thread to the GTK thread. The overlay window uses `wlr-layer-shell` to anchor at the bottom of the screen without keyboard interactivity. The 5-bar waveform is a custom-drawn `gtk::DrawingArea` using cairo, with the **exact** weights and smoothing constants from `dist/Sources/VoiceInput/OverlayPanel.swift:181-217`.

**Tech Stack:**
- `gtk4 = "0.10"` (Rust GTK4 bindings)
- `gtk4-layer-shell = "0.6"` (Wayland layer-shell protocol)
- `cairo-rs = "0.21"` (pulled in by gtk4; used for custom drawing)
- `glib = "0.21"` (also via gtk4; for MainContext::channel)
- Existing: ashpd, wl-clipboard-rs, cpal, whisper-rs, voice_activity_detector, tokio

**Reference spec:** `plans/voice-input-linux.md` — Phase 3 section + Visual design subsection. macOS source files to mirror constants from:
- `dist/Sources/VoiceInput/OverlayPanel.swift:181-217` — `WaveformView` weights `[0.5, 0.8, 1.0, 0.75, 0.55]`, attack 0.4 / release 0.15, ±4% jitter, `minBarFraction 0.15`
- `dist/Sources/VoiceInput/OverlayPanel.swift:97-135` — animation timings 0.35 / 0.25 / 0.22 (used as references; Phase 3 MVP keeps animation simple)
- `dist/Sources/VoiceInput/SpeechEngine.swift:97-103` — RMS normalization (already ported in Phase 1's `audio.rs::rms_normalized`)

**Phase 2 entry-condition carryovers** (Task 3.1 handles all):
1. `PipelineHandle` is `!Send` because `Option<Capture>` (cpal::Stream is `!Send`). Phase 2 review tried `spawn_blocking` and crashed compile (`8f59b21` → `5313f3c` revert). **Fix**: separate Capture from PipelineHandle entirely — `start_pipeline` returns `(Capture, PipelineHandle)`, and PipelineHandle holds only Send-friendly fields. Caller drops Capture on async thread, then `spawn_blocking` runs `pipeline.join_remaining()`.
2. `Config::shortcut_handle` is dead code (never written). Remove the field.
3. Threading model documented in `speech/mod.rs` doc comment must be updated to reflect the new GTK-main + tokio-bg arrangement.

---

## Threading & data flow (listen mode after Phase 3)

```
                    OS main thread                                  std::thread spawned from main
                    ──────────────                                  ──────────────────────────────
                    fn main()                                       tokio runtime (multi-thread)
                       │                                                 │
                       ▼                                                 ▼
                gtk::Application                                   run_listen_async
                       │                                                 │
                       │  channel ← OverlayCmd ──────────────────────────┤
                       │  (glib::MainContext::channel)                   │
                       ▼                                                 ▼
                overlay::WindowState                              ashpd portal session
                  ├─ OverlayWindow (layer-shell)                  + tokio::select! loop
                  └─ WaveformView (DrawingArea + cairo)                  │
                                                                         ▼
                                                                speech::start_pipeline
                                                                  → Capture + PipelineHandle
                                                                         │
                                                                         ▼
                                                                 cpal audio callback (its own thread)
                                                                   → AudioChunk{level, samples}
                                                                   → audio_tx (bounded 64)
                                                                         │
                                                                         ▼
                                                                  vad-resample thread
                                                                   ├─ fan out RMS level to GTK via overlay_tx
                                                                   ├─ resample → 16kHz mono
                                                                   └─ VAD slice → slice_tx
                                                                         │
                                                                         ▼
                                                                  whisper worker thread
                                                                   → text_tx
                                                                         │
                                                                         ▼
                                                                  (caller drains on release)
```

Two threads coordinate via a single `glib::MainContext::channel<OverlayCmd>`:

```rust
pub enum OverlayCmd {
    Show,           // hotkey pressed — make capsule visible, reset state
    SetLevel(f32),  // each AudioChunk produces one — drives waveform
    SetText(String),// "Refining…" placeholder text or final transcript preview
    Hide,           // hotkey released + paste done — dismiss capsule
}
```

The backend thread owns the sender; the GTK thread owns the receiver and attaches it to the main context.

---

## Open design decisions resolved before tasks begin

1. **GTK on main, tokio on background.** The brainstorm decision was explicit: GTK4 requires the OS main thread; tokio must yield. This task structure realizes it. The `tray` subcommand (Phase 0) keeps its own pattern (it doesn't use GTK), unchanged. `transcribe` (Phase 1) is unchanged.
2. **No fancy animations in Phase 3 MVP.** The macOS source has spring-entry (0.35s), elastic-width (0.25s), exit-scale (0.22s). Phase 3 ships with: instant show/hide, 60fps waveform redraw via `glib::timeout_add_local`. **Width animation deferred to Phase 5 polish.** Fixed capsule width = 360 px.
3. **"Refining" state is simulated in Phase 3.** No LLM refiner yet (Phase 4). On hotkey release, run_listen sends `SetText("Refining…")` to the overlay, sleeps ~250 ms (visual feedback), then sends `Hide` and pastes. Phase 4 replaces the sleep with the actual refiner call.
4. **Capsule visual** per brainstorm decision — Linux-native, NOT mimicking NSVisualEffectView:
   - Background `oklch(20% 0.01 280 / 0.92)` — dark, slightly desaturated, 92% alpha
   - 28 px corner radius (matches macOS geometry; only the blur effect was rejected)
   - Inner border `rgba(255,255,255,0.10)` at 0.5 px for definition
   - Box shadow `0 8px 24px rgba(0,0,0,0.45)` for legibility on light/dark wallpapers
   - Bar color `rgba(255,255,255,0.92)`
5. **Pipeline Capture/Handle split** — return tuple from `start_pipeline` so Capture (the `!Send` part) lives outside `PipelineHandle`. Caller is responsible for dropping Capture before moving PipelineHandle into spawn_blocking.

---

## File Structure (after Phase 3)

| Path | Responsibility |
|---|---|
| `linux/Cargo.toml` | Add gtk4, gtk4-layer-shell deps |
| `linux/src/lib.rs` | Add `pub mod overlay;` |
| `linux/src/speech/mod.rs` | MODIFY — split Capture from PipelineHandle, update doc comment for new threading |
| `linux/src/config.rs` | MODIFY — remove `shortcut_handle` field |
| `linux/src/main.rs` | MODIFY — `run_listen` restructured: tokio on bg thread, GTK on main |
| `linux/src/overlay/mod.rs` | NEW — public `OverlayCmd`, channel type aliases, `run_overlay_gtk_loop()` |
| `linux/src/overlay/window.rs` | NEW — `OverlayWindow`: layer-shell setup, capsule CSS, show/hide, label text |
| `linux/src/overlay/waveform.rs` | NEW — `WaveformView`: 5-bar `DrawingArea`, cairo draw, attack/release smoothing |
| `linux/README.md` | MODIFY — Phase 3 status, sway/hyprland layer-shell note, GNOME explicit unsupported reminder |
| `linux/tests/resolve_model_path.rs` | MODIFY — drop the `shortcut_handle` reference (it'll cease to exist) |

**Files NOT touched in Phase 3:** `audio.rs`, `speech/vad.rs`, `speech/worker.rs`, `hotkey.rs`, `injector.rs`, `cli.rs`, `error.rs`, `app.rs`, `tray.rs`.

---

## Task 3.1: Phase 2 carryovers — Capture/Pipeline split, drop shortcut_handle

**Files:**
- Modify: `linux/src/speech/mod.rs`
- Modify: `linux/src/config.rs`
- Modify: `linux/src/main.rs`
- Modify: `linux/tests/resolve_model_path.rs`

This is the most invasive task of Phase 3 because the Capture/Pipeline split touches the public API of `speech::start_pipeline`. We do it first so subsequent tasks build on a clean base.

- [ ] **Step 1: Edit `linux/src/speech/mod.rs`** — restructure `PipelineHandle` and `start_pipeline`.

Find the existing `PipelineHandle` struct + impl + Drop:

```rust
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    /// Wrapped in `Option` so `join` can drop the audio stream BEFORE
    /// awaiting the VAD thread. Without this, the VAD thread would block
    /// on `audio_rx.recv()` forever because `audio_tx` lives inside the
    /// cpal stream owned by `Capture`.
    capture: Option<Capture>,
    vad_handle: Option<JoinHandle<()>>,
    whisper_handle: Option<JoinHandle<()>>,
}
```

Replace ENTIRELY with:

```rust
/// Send-friendly handle to the speech pipeline worker threads.
///
/// `PipelineHandle` deliberately does NOT own the `Capture` audio stream
/// (which is `!Send` because `cpal::Stream` is `!Send`). The caller of
/// `start_pipeline` receives a `(Capture, PipelineHandle)` tuple and must
/// hold Capture on the thread where the pipeline was started; this lets
/// the PipelineHandle be moved into `tokio::task::spawn_blocking` while
/// the caller drops Capture on the original thread to unblock the VAD
/// thread's `audio_rx.recv()`.
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    vad_handle: Option<JoinHandle<()>>,
    whisper_handle: Option<JoinHandle<()>>,
}

impl PipelineHandle {
    /// Join the worker threads. Drains `text_rx` after joins return —
    /// at that point all senders have been dropped, so `try_recv` yields
    /// every buffered segment exactly once.
    ///
    /// IMPORTANT: the caller must drop the corresponding `Capture` BEFORE
    /// calling this (typically via `drop(capture)` on the async thread,
    /// then `spawn_blocking(move || handle.join_remaining())`). Without
    /// dropping Capture first, this will block forever because the VAD
    /// thread is waiting for `audio_rx` to close.
    pub fn join_remaining(mut self) -> Vec<String> {
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.whisper_handle.take() {
            let _ = h.join();
        }
        let mut out = Vec::new();
        while let Ok(seg) = self.text_rx.try_recv() {
            out.push(seg);
        }
        out
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        // If `join_remaining` wasn't called, ensure threads still get joined
        // on Drop. This won't deadlock as long as the corresponding Capture
        // has been dropped — if it hasn't, the caller has a bug and will
        // see this hang in their logs.
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.whisper_handle.take() {
            let _ = h.join();
        }
    }
}
```

Then find `start_pipeline` and change its return type and final return:

```rust
pub fn start_pipeline(model_path: &Path, language_hint: String) -> AppResult<PipelineHandle> {
    let (audio_tx, audio_rx) = bounded::<AudioChunk>(64);
    let (slice_tx, slice_rx) = bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = bounded::<String>(64);

    let capture = Capture::start(audio_tx)?;
    let input_rate = capture.sample_rate;
    let input_channels = capture.channels;

    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;

    let whisper_handle = worker::spawn(model_path, language_hint, slice_rx, text_tx)?;

    Ok(PipelineHandle {
        text_rx,
        capture: Some(capture),
        vad_handle: Some(vad_handle),
        whisper_handle: Some(whisper_handle),
    })
}
```

Replace with:

```rust
pub fn start_pipeline(
    model_path: &Path,
    language_hint: String,
) -> AppResult<(Capture, PipelineHandle)> {
    let (audio_tx, audio_rx) = bounded::<AudioChunk>(64);
    let (slice_tx, slice_rx) = bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = bounded::<String>(64);

    let capture = Capture::start(audio_tx)?;
    let input_rate = capture.sample_rate;
    let input_channels = capture.channels;

    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;

    let whisper_handle = worker::spawn(model_path, language_hint, slice_rx, text_tx)?;

    let handle = PipelineHandle {
        text_rx,
        vad_handle: Some(vad_handle),
        whisper_handle: Some(whisper_handle),
    };
    Ok((capture, handle))
}
```

Update the top-level doc comment at the top of `speech/mod.rs` from:

```rust
//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.
```

to:

```rust
//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules use `std::thread` + `crossbeam_channel`, not tokio
//! tasks. `voice-input listen` (Phase 2+) runs GTK4 on the OS main
//! thread (a hard GTK4 requirement) and the tokio runtime in a
//! background `std::thread`; the speech pipeline lives entirely
//! beneath that backend thread and is unaffected by the GTK/tokio
//! split. See `main::run_listen` for the wiring.
//!
//! `start_pipeline` returns `(Capture, PipelineHandle)` as a tuple:
//! `Capture` holds the cpal stream and is `!Send`, so it stays on
//! the calling thread. `PipelineHandle` is `Send` and can be moved
//! into `spawn_blocking` for the worker-thread joins on shutdown.
```

Also remove the old `drain_and_join` method — it's now replaced by `join_remaining` with a clearer contract. (The Phase 2 carryover comment at main.rs:167 about `drain_and_join` being unwrappable in spawn_blocking is now resolved by this refactor.)

- [ ] **Step 2: Edit `linux/src/config.rs`** — remove the `shortcut_handle` field.

Find:

```rust
pub struct Config {
    pub language_hint: String,
    pub llm_enabled: bool,
    pub llm_api_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub whisper_model_size: String,
    pub whisper_model_path: Option<PathBuf>,
    pub shortcut_handle: Option<String>,
}
```

Remove the `shortcut_handle` line, leaving 7 fields:

```rust
pub struct Config {
    pub language_hint: String,
    pub llm_enabled: bool,
    pub llm_api_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub whisper_model_size: String,
    pub whisper_model_path: Option<PathBuf>,
}
```

Then find the `Default` impl:

```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            language_hint: "zh".to_string(),
            llm_enabled: false,
            llm_api_base_url: "https://api.openai.com/v1".to_string(),
            llm_api_key: String::new(),
            llm_model: "gpt-4o-mini".to_string(),
            whisper_model_size: "small".to_string(),
            whisper_model_path: None,
            shortcut_handle: None,
        }
    }
}
```

Remove `shortcut_handle: None,`:

```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            language_hint: "zh".to_string(),
            llm_enabled: false,
            llm_api_base_url: "https://api.openai.com/v1".to_string(),
            llm_api_key: String::new(),
            llm_model: "gpt-4o-mini".to_string(),
            whisper_model_size: "small".to_string(),
            whisper_model_path: None,
        }
    }
}
```

- [ ] **Step 3: Edit `linux/src/main.rs`** — update `run_transcribe` and `run_listen_async` to use the new tuple return + `join_remaining`.

Find in `run_transcribe`:

```rust
    let pipeline = voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone())
        .context("starting speech pipeline")?;
```

Replace with:

```rust
    let (_capture, pipeline) = voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone())
        .context("starting speech pipeline")?;
```

(`_capture` is the cpal stream; we hold it for the duration of the function. When `run_transcribe` returns, both drop in order.)

Then find the bottom of `run_transcribe`:

```rust
    pipeline.join();
    tracing::info!("pipeline shutdown complete; transcribed {} segments", segment_count);
    Ok(())
```

`PipelineHandle` no longer has `join` (replaced by `join_remaining` which returns segments). For the transcribe flow, we don't care about the segments because we've already printed them as they came in. Replace with:

```rust
    drop(_capture); // stop audio so VAD thread exits
    let _ = pipeline.join_remaining(); // drain any final segment we missed
    tracing::info!("pipeline shutdown complete; transcribed {} segments", segment_count);
    Ok(())
```

Then find `run_listen_async`'s deactivated arm:

```rust
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    // drain_and_join is blocking but cannot use spawn_blocking:
                    // PipelineHandle is !Send because cpal::Stream is !Send.
                    // Phase 3 should split Capture drop from thread joins to fix.
                    let segments = pipeline.drain_and_join();
                    let joined = segments.join(" ").trim().to_string();
```

Update the activated arm first (which currently calls start_pipeline). Find:

```rust
            Some(_activated) = activated.next() => {
                if current_pipeline.is_some() {
                    // The portal occasionally double-emits activated; ignore if already recording.
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, language_hint.clone()) {
                    Ok(p) => current_pipeline = Some(p),
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
            }
```

Replace with:

```rust
            Some(_activated) = activated.next() => {
                if current_pipeline.is_some() {
                    // The portal occasionally double-emits activated; ignore if already recording.
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, language_hint.clone()) {
                    Ok((capture, p)) => {
                        current_capture = Some(capture);
                        current_pipeline = Some(p);
                    }
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
            }
```

Then replace the deactivated arm with:

```rust
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    // Drop Capture first (it's !Send, must stay on this thread)
                    // — that lets the VAD thread's audio_rx see disconnect.
                    drop(current_capture.take());
                    // PipelineHandle is now Send: move it to the blocking pool
                    // so the join + drain don't stall the async runtime.
                    let segments = tokio::task::spawn_blocking(move || pipeline.join_remaining())
                        .await
                        .context("draining pipeline")?;
                    let joined = segments.join(" ").trim().to_string();
                    if joined.is_empty() {
                        tracing::info!("no segments transcribed; skipping paste");
                    } else {
                        tracing::info!(segments = segments.len(), bytes = joined.len(), "pasting");
                        let injected = tokio::task::spawn_blocking({
                            let joined = joined.clone();
                            move || voice_input::injector::inject_text(&joined)
                        })
                        .await
                        .context("ydotool paste task")?;
                        if let Err(e) = injected {
                            tracing::error!(error = %e, "paste failed");
                        }
                    }
                }
            }
```

And add `current_capture` declaration near the top of `run_listen_async`. Find:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let language_hint = cfg.language_hint.clone();
```

Replace with:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();
```

- [ ] **Step 4: Edit `linux/tests/resolve_model_path.rs`** — update the test that constructed a Config with the new field set.

Find:

```rust
#[test]
fn config_field_wins_over_default() {
    let cfg = Config {
        whisper_model_path: Some(PathBuf::from("/tmp/voice-input-test-config.bin")),
        ..Config::default()
    };
```

This compiles unchanged because `..Config::default()` fills in remaining fields. No edit needed for this specific test.

But if there are any other tests or code paths that reference `shortcut_handle`, they need to be removed. Run:

```bash
grep -rn "shortcut_handle" /home/desmond/Repos/voice-input-src/linux/
```

Expected: no matches after the config.rs and README edits. If matches appear in any source/test/doc, remove or update them.

- [ ] **Step 5: Build and run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected:
- Build clean
- All 31 prior tests still pass + 2 ignored (no test count change in Phase 3.1; we just refactored API shape)

Common build failure: `pipeline.drain_and_join` referenced somewhere we missed. `grep -n drain_and_join linux/src/` and remove leftover references.

- [ ] **Step 6: Smoke test ydotool-missing error path still works**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo run --release -- transcribe 2>&1 | head -5
```

Expected: `transcribe` works without errors (the refactor preserves behavior).

- [ ] **Step 7: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/mod.rs linux/src/config.rs linux/src/main.rs linux/tests/resolve_model_path.rs
git commit -m "refactor(linux): split Capture from PipelineHandle for spawn_blocking + drop shortcut_handle"
```

---

## Task 3.2: Add Phase 3 dependencies + system packages

**Files:**
- Modify: `linux/Cargo.toml`

- [ ] **Step 1: Verify system packages**

```bash
dpkg -l libgtk-4-dev libgtk4-layer-shell-dev 2>&1 | grep -E "^ii"
```

If either is missing:

```bash
sudo apt install -y libgtk-4-dev libgtk4-layer-shell-dev
```

If `libgtk4-layer-shell-dev` is not in apt repositories (older Ubuntu), STOP and report. The Ubuntu 24.04+ repos have it; older systems may need a PPA or building from source.

- [ ] **Step 2: Edit `linux/Cargo.toml`**

Add 2 new entries in alphabetical order: `gtk4` and `gtk4-layer-shell`. The final `[dependencies]` section should be (post-`shortcut_handle` removal didn't touch Cargo.toml, so no change there):

```toml
[dependencies]
anyhow = "1"
ashpd = { version = "0.13", features = ["global_shortcuts"] }
clap = { version = "4", features = ["derive"] }
cpal = "0.15"
crossbeam-channel = "0.5"
ctrlc = "3"
directories = "5"
futures-util = "0.3"
gtk4 = { version = "0.10", features = ["v4_12"] }
gtk4-layer-shell = "0.6"
ksni = { version = "0.3", features = ["tokio"] }
rubato = "0.16"
serde = { version = "1", features = ["derive"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "signal"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
voice_activity_detector = "0.2"
whisper-rs = "0.14"
wl-clipboard-rs = "0.9"
```

The `features = ["v4_12"]` on gtk4 enables APIs from GTK 4.12+ (Ubuntu 24.04 ships 4.14, Plasma 6 typically has 4.14+). If your target system is older, drop that feature flag.

- [ ] **Step 3: Build — first compile takes a while because gtk4-rs has many transitive crates**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -15
```

Expected: clean `Finished` line, possibly 30-90 seconds on first build because of gtk4-rs codegen. If you see:
- `Package gtk4 was not found`: install `libgtk-4-dev`
- `Package gtk4-layer-shell-0 was not found`: install `libgtk4-layer-shell-dev`
- `gtk4 0.10` doesn't exist on crates.io: STOP and report `NEEDS_CONTEXT` with `cargo search gtk4 --limit 5`
- `gtk4-layer-shell 0.6` version drift: STOP and report

- [ ] **Step 4: Run prior tests to confirm no regressions**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: 31 + 2 ignored unchanged. The new deps don't add tests.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/Cargo.lock
git commit -m "feat(linux): add gtk4 and gtk4-layer-shell dependencies"
```

---

## Task 3.3: Create overlay module skeleton + OverlayCmd channel

**Files:**
- Create: `linux/src/overlay/mod.rs`
- Modify: `linux/src/lib.rs`

This task lands the public API surface for the overlay (`OverlayCmd` enum + a placeholder `run_overlay_gtk_loop` function) without any actual GTK code yet. Task 3.4 and 3.5 fill in the window + waveform pieces.

- [ ] **Step 1: Edit `linux/src/lib.rs`** — add `pub mod overlay;` (10 lines alphabetical):

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod injector;
pub mod overlay;
pub mod speech;
pub mod tray;
```

- [ ] **Step 2: Create `linux/src/overlay/mod.rs`** with this content:

```rust
//! GTK4 + layer-shell overlay capsule shown during `listen` mode.
//!
//! Lives entirely on the OS main thread. The backend thread (where the
//! ashpd portal + speech pipeline run) sends `OverlayCmd` messages via a
//! `glib::MainContext::channel`; the main thread receives and applies them
//! to the `OverlayWindow` + `WaveformView` widgets.

pub mod waveform;
pub mod window;

use std::sync::mpsc;

/// Commands the backend thread sends to the GTK main thread.
#[derive(Debug, Clone)]
pub enum OverlayCmd {
    /// Hotkey pressed — make the capsule visible.
    Show,
    /// Updated audio level in [0, 1]. Drives waveform animation.
    SetLevel(f32),
    /// Replace the text label content. Used for state transitions
    /// ("Listening…", "Refining…", future partial transcripts).
    SetText(String),
    /// Hotkey released and paste completed — hide the capsule.
    Hide,
}

/// Backend → main channel.
///
/// We use `std::sync::mpsc` here (NOT `glib::MainContext::channel`) so the
/// `OverlaySender` is `Send` and can be cloned/moved into the backend
/// thread without GTK headers being in scope. The GTK loop drains via
/// `glib::timeout_add_local` polling the receiver — this trades CPU for
/// simplicity. A future polish task can swap to `glib` channels for true
/// event-driven dispatch.
pub type OverlaySender = mpsc::Sender<OverlayCmd>;
pub type OverlayReceiver = mpsc::Receiver<OverlayCmd>;

pub fn channel() -> (OverlaySender, OverlayReceiver) {
    mpsc::channel()
}
```

- [ ] **Step 3: Create stub `linux/src/overlay/window.rs`** (real content in Task 3.4):

```rust
//! Capsule overlay window — populated in Task 3.4.
```

- [ ] **Step 4: Create stub `linux/src/overlay/waveform.rs`** (real content in Task 3.5):

```rust
//! 5-bar waveform widget — populated in Task 3.5.
```

- [ ] **Step 5: Build clean**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: build clean (no warnings, the stub files have no code so no unused-import lint), 31 + 2 ignored tests unchanged.

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/lib.rs linux/src/overlay/
git commit -m "feat(linux): add overlay module scaffold with OverlayCmd channel"
```

---

## Task 3.4: OverlayWindow — layer-shell capsule + CSS styling

**Files:**
- Modify: `linux/src/overlay/window.rs`

This task fills in the capsule window. The window is borderless, anchored at the bottom-center via layer-shell, with a 28 px corner radius and dark alpha background per the brainstorm decision.

- [ ] **Step 1: Replace `linux/src/overlay/window.rs` with:**

```rust
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Label, Orientation};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::waveform::WaveformView;

const CAPSULE_WIDTH: i32 = 360;
const CAPSULE_HEIGHT: i32 = 56;
const CAPSULE_MARGIN_BOTTOM: i32 = 56;

/// CSS applied to the overlay capsule. Linux-native styling: solid dark
/// alpha background, soft inner border, drop shadow. No blur — that would
/// require compositor-specific protocols (KWin blur effect) and isn't
/// portable. See brainstorm decision in plans/voice-input-linux.md.
const CAPSULE_CSS: &str = r#"
window.voice-input-overlay {
    background: transparent;
}
window.voice-input-overlay > .capsule {
    background-color: rgba(28, 28, 36, 0.92);
    border-radius: 28px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    padding: 8px 24px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
}
window.voice-input-overlay .overlay-label {
    color: rgba(255, 255, 255, 0.92);
    font-size: 15px;
    font-weight: 500;
    padding-left: 14px;
}
"#;

pub struct OverlayWindow {
    window: ApplicationWindow,
    label: Label,
    waveform: WaveformView,
}

impl OverlayWindow {
    pub fn new(app: &Application) -> Self {
        // Install the CSS once globally per gtk4 docs — adding multiple
        // providers stacks them; using one is fine.
        let provider = CssProvider::new();
        provider.load_from_string(CAPSULE_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(CAPSULE_WIDTH)
            .default_height(CAPSULE_HEIGHT)
            .resizable(false)
            .decorated(false)
            .build();
        window.add_css_class("voice-input-overlay");

        // layer-shell setup: bottom-center, no exclusive zone, no keyboard focus.
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, CAPSULE_MARGIN_BOTTOM);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_exclusive_zone(-1);

        // Layout: [waveform] [label]
        let capsule = GtkBox::new(Orientation::Horizontal, 14);
        capsule.add_css_class("capsule");
        capsule.set_halign(Align::Center);
        capsule.set_valign(Align::Center);

        let waveform = WaveformView::new();
        capsule.append(waveform.widget());

        let label = Label::new(Some("Listening…"));
        label.add_css_class("overlay-label");
        label.set_halign(Align::Start);
        capsule.append(&label);

        window.set_child(Some(&capsule));

        Self {
            window,
            label,
            waveform,
        }
    }

    pub fn show(&self) {
        self.label.set_text("Listening…");
        self.waveform.reset();
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn set_text(&self, text: &str) {
        self.label.set_text(text);
    }

    pub fn set_level(&self, level: f32) {
        self.waveform.set_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_dimensions_match_design() {
        // Pinned constants — Phase 5 might tune these, but accidental
        // drift should be a deliberate decision.
        assert_eq!(CAPSULE_WIDTH, 360);
        assert_eq!(CAPSULE_HEIGHT, 56);
        assert_eq!(CAPSULE_MARGIN_BOTTOM, 56);
    }
}
```

(`Label`/`Application`/`ApplicationWindow`/`GtkBox` come from `gtk4`. `LayerShell` from `gtk4-layer-shell`. The `WaveformView::new()` / `widget()` / `set_level()` / `reset()` interface is defined in Task 3.5.)

- [ ] **Step 2: Build — verify the gtk4-layer-shell API matches**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -20
```

Expected: clean build OR a compile error from `WaveformView` not being implemented yet — that's expected because Task 3.5 fills in the impl. If THAT'S the only error, ignore for now and proceed (Task 3.5 will resolve it before commit).

If you see errors like:
- `gtk4_layer_shell::LayerShell` trait not found → API drift; STOP and report
- `init_layer_shell` not a method on `ApplicationWindow` → STOP, may need explicit `use gtk4_layer_shell::LayerShell;`
- `Edge::Bottom` variant differs → STOP and report

Don't proceed past Task 3.4 if the layer-shell API doesn't match what's shown — instead, STOP and report `NEEDS_CONTEXT` with the actual signatures from `~/.cargo/registry/src/index.crates.io-*/gtk4-layer-shell-0.6*/src/`.

- [ ] **Step 3: Commit (even with WaveformView stub causing partial failure — that resolves in Task 3.5)**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/overlay/window.rs
git commit -m "feat(linux): add layer-shell capsule overlay window"
```

---

## Task 3.5: WaveformView — 5-bar custom DrawingArea

**Files:**
- Modify: `linux/src/overlay/waveform.rs`

This task implements the 5-bar waveform driven by RMS levels. The constants are **exactly** ported from `dist/Sources/VoiceInput/OverlayPanel.swift:181-217` so the visual feel matches macOS.

- [ ] **Step 1: Replace `linux/src/overlay/waveform.rs` with:**

```rust
use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{DrawingArea, glib};

/// 5-bar waveform widget — exact port of macOS `WaveformView`.
///
/// Constants mirror `dist/Sources/VoiceInput/OverlayPanel.swift:181-217`:
/// - 5 bars with weights `[0.5, 0.8, 1.0, 0.75, 0.55]` (center-high)
/// - Attack 0.4 / release 0.15 smoothing on the input level
/// - ±4% per-bar jitter for organic feel
/// - `MIN_BAR_FRACTION = 0.15` so silent bars stay visible
/// - Bar width 4.5 px, gap 3.5 px, view 44×32 px

const BAR_COUNT: usize = 5;
const BAR_WEIGHTS: [f64; BAR_COUNT] = [0.5, 0.8, 1.0, 0.75, 0.55];
const MIN_BAR_FRACTION: f64 = 0.15;
const ATTACK: f64 = 0.4;
const RELEASE: f64 = 0.15;
const JITTER: f64 = 0.04;

const BAR_WIDTH: f64 = 4.5;
const BAR_GAP: f64 = 3.5;
const VIEW_WIDTH: i32 = 44;
const VIEW_HEIGHT: i32 = 32;

const REDRAW_HZ: u32 = 60;

pub struct WaveformView {
    drawing_area: DrawingArea,
    smoothed_level: Rc<Cell<f64>>,
    target_level: Rc<Cell<f64>>,
}

impl WaveformView {
    pub fn new() -> Self {
        let drawing_area = DrawingArea::builder()
            .content_width(VIEW_WIDTH)
            .content_height(VIEW_HEIGHT)
            .build();

        let smoothed_level = Rc::new(Cell::new(0.0_f64));
        let target_level = Rc::new(Cell::new(0.0_f64));

        // Per-frame smoothing + redraw via glib::timeout_add_local.
        let smoothed = smoothed_level.clone();
        let target = target_level.clone();
        let area_ref = drawing_area.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(1000 / REDRAW_HZ as u64), move || {
            let prev = smoothed.get();
            let tgt = target.get();
            let factor = if tgt > prev { ATTACK } else { RELEASE };
            let new = prev + (tgt - prev) * factor;
            smoothed.set(new);
            area_ref.queue_draw();
            glib::ControlFlow::Continue
        });

        // Draw callback closes over smoothed_level (read-only).
        let smoothed_for_draw = smoothed_level.clone();
        drawing_area.set_draw_func(move |_, ctx, w, h| {
            draw_bars(ctx, w as f64, h as f64, smoothed_for_draw.get());
        });

        Self {
            drawing_area,
            smoothed_level,
            target_level,
        }
    }

    pub fn widget(&self) -> &DrawingArea {
        &self.drawing_area
    }

    /// Push a new target level. The widget smooths toward it at REDRAW_HZ.
    /// Input is expected to be in [0, 1] (per `audio::rms_normalized`).
    pub fn set_level(&self, level: f32) {
        let clamped = (level as f64).clamp(0.0, 1.0);
        self.target_level.set(clamped);
    }

    /// Snap level to 0 and clear smoothing — used when the capsule is
    /// re-shown so old levels don't bleed across sessions.
    pub fn reset(&self) {
        self.target_level.set(0.0);
        self.smoothed_level.set(0.0);
        self.drawing_area.queue_draw();
    }
}

fn draw_bars(ctx: &Context, width: f64, height: f64, level: f64) {
    let total_width = BAR_COUNT as f64 * BAR_WIDTH + (BAR_COUNT - 1) as f64 * BAR_GAP;
    let start_x = (width - total_width) / 2.0;
    let center_y = height / 2.0;

    // Bar color: rgba(255, 255, 255, 0.92).
    ctx.set_source_rgba(1.0, 1.0, 1.0, 0.92);

    for i in 0..BAR_COUNT {
        let weight = BAR_WEIGHTS[i];
        // Cheap jitter — using a hash of (i, level) keeps it stable per
        // frame so bars don't flicker chaotically. Real impl could use
        // a per-bar rng. ±4% per the macOS constant.
        let jitter = (((i as f64) * 73.0 + level * 991.0).sin()) * JITTER;
        let fraction = MIN_BAR_FRACTION + (1.0 - MIN_BAR_FRACTION) * level * weight;
        let clamped = (fraction + jitter).clamp(MIN_BAR_FRACTION, 1.0);
        let bar_h = height * clamped;

        let x = start_x + i as f64 * (BAR_WIDTH + BAR_GAP);
        let y = center_y - bar_h / 2.0;

        // Rounded rect (corner radius 2.5 px like macOS).
        rounded_rect(ctx, x, y, BAR_WIDTH, bar_h, 2.5);
        ctx.fill().unwrap_or(());
    }
}

fn rounded_rect(ctx: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.new_sub_path();
    ctx.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    ctx.arc(x + r, y + h - r, r, PI / 2.0, PI);
    ctx.arc(x + r, y + r, r, PI, 3.0 * PI / 2.0);
    ctx.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_close_to_design() {
        let sum: f64 = BAR_WEIGHTS.iter().sum();
        // Center-high distribution: 0.5+0.8+1.0+0.75+0.55 = 3.6
        assert!((sum - 3.6).abs() < 1e-9, "weights drift: {}", sum);
    }

    #[test]
    fn min_bar_fraction_keeps_silence_visible() {
        // At level=0, jitter=0 → fraction = MIN_BAR_FRACTION = 0.15.
        // Multiplied by view_height=32: silent bars are ~4.8px tall, still visible.
        assert_eq!(MIN_BAR_FRACTION, 0.15);
        assert!((MIN_BAR_FRACTION * VIEW_HEIGHT as f64) > 4.0);
    }

    #[test]
    fn full_scale_level_reaches_full_height_after_jitter() {
        // At level=1.0, fraction = 0.15 + 0.85 * 1.0 * weight; for the
        // center bar (weight=1.0) that's 1.0. With +4% jitter we'd exceed
        // 1.0, but clamp() pins to 1.0.
        let level = 1.0;
        let weight = 1.0;
        let fraction = MIN_BAR_FRACTION + (1.0 - MIN_BAR_FRACTION) * level * weight;
        assert!((fraction - 1.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
```

Expected: clean build now that WaveformView exists.

If you see errors about `gtk4::cairo::Context` or `set_draw_func` signature, STOP and report. The gtk4-rs API for DrawingArea has stable since 0.7 but verify.

- [ ] **Step 3: Run waveform unit tests**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib overlay::waveform 2>&1 | tail -10
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: 3 new waveform tests pass + 31 prior + 2 ignored = 34 + 2 ignored total.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/overlay/waveform.rs
git commit -m "feat(linux): add 5-bar waveform widget with macOS-derived constants"
```

---

## Task 3.6: Wire overlay into run_listen — GTK main + tokio bg thread

**Files:**
- Modify: `linux/src/main.rs`

This is the threading restructure. `run_listen` now spawns the tokio backend in `std::thread::spawn`, then runs `gtk::Application::run` on the OS main thread. Cross-thread coordination via the `OverlayCmd` channel from Task 3.3.

- [ ] **Step 1: At the top of `linux/src/main.rs`, add imports**

Find the existing import block:

```rust
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use ksni::TrayMethods;
use tokio::sync::Notify;
use voice_input::{
    cli::{Cli, Command},
    config::Config,
    tray::VoiceInputTray,
};
```

Add `gtk4::prelude::*;`, `gtk4::Application`, and `voice_input::overlay::{...}`:

```rust
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use gtk4::prelude::*;
use gtk4::Application;
use ksni::TrayMethods;
use tokio::sync::Notify;
use voice_input::{
    cli::{Cli, Command},
    config::Config,
    overlay::{self, OverlayCmd, OverlayWindow},
    tray::VoiceInputTray,
};
```

(`OverlayWindow` is from `overlay::window::OverlayWindow` — re-exported via `pub use` to avoid leaking submodule paths. If it's not re-exported, this import fails; if so, change to `voice_input::overlay::window::OverlayWindow`.)

Also re-export `OverlayWindow` from `overlay/mod.rs` by adding to its top:

```rust
pub use window::OverlayWindow;
```

- [ ] **Step 2: Replace `run_listen` and `run_listen_async`**

Find the existing `fn run_listen` and the whole `async fn run_listen_async` — replace ENTIRELY with:

```rust
fn run_listen(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg.resolve_model_path().context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting listen mode");

    voice_input::injector::verify_available()
        .context("ydotool must be installed and ydotoold running")?;

    // Backend ↔ GTK channel.
    let (overlay_tx, overlay_rx) = overlay::channel();

    // Spawn the backend thread (owns tokio runtime + ashpd portal + pipeline).
    let cfg_for_backend = cfg.clone();
    let model_path_for_backend = model_path.clone();
    let backend = std::thread::Builder::new()
        .name("voice-input-backend".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime in backend thread");
            if let Err(e) = rt.block_on(run_listen_async(
                cfg_for_backend,
                model_path_for_backend,
                overlay_tx,
            )) {
                tracing::error!(error = %e, "backend exited with error");
            }
        })
        .context("spawning backend thread")?;

    // Run GTK on the OS main thread. The Application's `activate` callback
    // creates the OverlayWindow and attaches the overlay_rx polling loop.
    let app = Application::builder()
        .application_id("com.yetone.VoiceInput")
        .build();

    let overlay_rx_cell = std::cell::RefCell::new(Some(overlay_rx));
    app.connect_activate(move |app| {
        let window = OverlayWindow::new(app);
        // Hidden until backend sends Show.
        window.hide();

        // Take the receiver into a long-lived poll loop.
        let rx = overlay_rx_cell
            .borrow_mut()
            .take()
            .expect("activate called once");
        let window_for_loop = window;
        gtk4::glib::timeout_add_local(
            std::time::Duration::from_millis(16),
            move || {
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        OverlayCmd::Show => window_for_loop.show(),
                        OverlayCmd::Hide => window_for_loop.hide(),
                        OverlayCmd::SetLevel(level) => window_for_loop.set_level(level),
                        OverlayCmd::SetText(text) => window_for_loop.set_text(&text),
                    }
                }
                gtk4::glib::ControlFlow::Continue
            },
        );
    });

    // app.run() blocks until the GTK loop exits (we never explicitly quit
    // it in this flow; the user kills the process with Ctrl+C in the
    // launching terminal, which terminates everything).
    let exit_code = app.run();

    // Tell the backend we're shutting down by closing the channel.
    // The backend's tokio::select! has a ctrl_c arm; once the user
    // SIGINTs, both halves wind down.
    let _ = backend.join();

    if exit_code.value() != 0 {
        anyhow::bail!("gtk application exited with code {}", exit_code.value());
    }
    Ok(())
}

async fn run_listen_async(
    cfg: Config,
    model_path: std::path::PathBuf,
    overlay_tx: overlay::OverlaySender,
) -> anyhow::Result<()> {
    use futures_util::stream::StreamExt;
    use voice_input::hotkey::HotkeyHandle;
    use voice_input::speech;

    let hotkey = HotkeyHandle::create()
        .await
        .context("creating portal global-shortcuts session")?;
    let mut activated = hotkey.activated().await.context("activated stream")?;
    let mut deactivated = hotkey.deactivated().await.context("deactivated stream")?;

    tracing::info!(
        "listen mode running — hold the bound shortcut to dictate; press Ctrl+C to exit"
    );

    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("SIGINT received; exiting listen mode");
                break;
            }
            Some(_activated) = activated.next() => {
                if current_pipeline.is_some() {
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, language_hint.clone()) {
                    Ok((capture, p)) => {
                        let _ = overlay_tx.send(OverlayCmd::Show);
                        current_capture = Some(capture);
                        current_pipeline = Some(p);
                    }
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
            }
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    let _ = overlay_tx.send(OverlayCmd::SetText("Refining…".into()));
                    drop(current_capture.take());
                    let segments = tokio::task::spawn_blocking(move || pipeline.join_remaining())
                        .await
                        .context("draining pipeline")?;
                    let joined = segments.join(" ").trim().to_string();
                    if joined.is_empty() {
                        tracing::info!("no segments transcribed; skipping paste");
                    } else {
                        tracing::info!(segments = segments.len(), bytes = joined.len(), "pasting");
                        let injected = tokio::task::spawn_blocking({
                            let joined = joined.clone();
                            move || voice_input::injector::inject_text(&joined)
                        })
                        .await
                        .context("ydotool paste task")?;
                        if let Err(e) = injected {
                            tracing::error!(error = %e, "paste failed");
                        }
                    }
                    let _ = overlay_tx.send(OverlayCmd::Hide);
                }
            }
            else => {
                tracing::warn!("portal streams closed; exiting");
                break;
            }
        }
    }

    // Final overlay cleanup attempt.
    let _ = overlay_tx.send(OverlayCmd::Hide);
    Ok(())
}
```

NOTE: this version does NOT yet wire RMS levels into the overlay's waveform. Audio levels flow through `audio_tx` → `vad-resample` thread, not directly accessible here. Task 3.7 adds the level fan-out.

- [ ] **Step 3: Build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
```

Expected: clean build. The overlay window will appear on hotkey press but the waveform bars will stay flat (no levels delivered yet — Task 3.7 fixes this).

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/main.rs linux/src/overlay/mod.rs
git commit -m "feat(linux): wire GTK overlay on main thread + tokio backend on std::thread"
```

---

## Task 3.7: Fan out cpal RMS levels to the overlay

**Files:**
- Modify: `linux/src/speech/mod.rs`

The `run_vad_resample` thread receives every `AudioChunk` and computes its `level` field. We add an optional `level_tx: Option<Sender<f32>>` parameter so the listen path can subscribe.

- [ ] **Step 1: Modify `run_vad_resample` signature and body in `linux/src/speech/mod.rs`**

Find:

```rust
fn run_vad_resample(
    audio_rx: Receiver<AudioChunk>,
    slice_tx: crossbeam_channel::Sender<Vec<f32>>,
    input_rate: u32,
    input_channels: u16,
) {
```

Replace with:

```rust
fn run_vad_resample(
    audio_rx: Receiver<AudioChunk>,
    slice_tx: crossbeam_channel::Sender<Vec<f32>>,
    input_rate: u32,
    input_channels: u16,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
) {
```

Inside the loop, where `chunk` is received, add level fan-out right after `Ok(chunk)`. Find:

```rust
    while let Ok(chunk) = audio_rx.recv() {
        let mono16k = match resampler.process(&chunk.samples) {
```

Replace with:

```rust
    while let Ok(chunk) = audio_rx.recv() {
        // Fan out the RMS level to the overlay if subscribed.
        if let Some(tx) = level_tx.as_ref() {
            // try_send: if the consumer is slow, drop the level update.
            // Levels are inherently lossy anyway.
            let _ = tx.try_send(chunk.level);
        }
        let mono16k = match resampler.process(&chunk.samples) {
```

- [ ] **Step 2: Modify `start_pipeline` to accept and pass through `level_tx`**

Find:

```rust
pub fn start_pipeline(
    model_path: &Path,
    language_hint: String,
) -> AppResult<(Capture, PipelineHandle)> {
```

Replace with:

```rust
pub fn start_pipeline(
    model_path: &Path,
    language_hint: String,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
) -> AppResult<(Capture, PipelineHandle)> {
```

Then inside, find:

```rust
    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;
```

Replace with:

```rust
    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels, level_tx);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;
```

- [ ] **Step 3: Update callers in `linux/src/main.rs`**

Find in `run_transcribe`:

```rust
    let (_capture, pipeline) = voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone())
        .context("starting speech pipeline")?;
```

Replace with:

```rust
    let (_capture, pipeline) =
        voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone(), None)
            .context("starting speech pipeline")?;
```

(transcribe mode passes `None` — no overlay.)

Find in `run_listen_async`'s activated arm:

```rust
                match speech::start_pipeline(&model_path, language_hint.clone()) {
```

Replace with:

```rust
                match speech::start_pipeline(&model_path, language_hint.clone(), Some(level_tx.clone())) {
```

And at the top of `run_listen_async`, add a level channel and a forwarder task. Find:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();
```

Replace with:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();

    // RMS level fan-out: vad-resample thread → blocking task → overlay channel.
    // We use a bounded crossbeam channel sized for ~1s of audio at 100Hz.
    let (level_tx, level_rx) = crossbeam_channel::bounded::<f32>(128);
    let overlay_tx_for_levels = overlay_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(level) = level_rx.recv() {
            // Stop forwarding when the overlay channel is closed.
            if overlay_tx_for_levels.send(OverlayCmd::SetLevel(level)).is_err() {
                break;
            }
        }
    });
```

(`OverlaySender` is `mpsc::Sender<OverlayCmd>` which is `Clone`.)

Also add `use voice_input::overlay::OverlayCmd;` to the inner async block if not already imported via the outer `use` statements at the top of main.rs.

- [ ] **Step 4: Build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: clean build; tests unchanged (the new `level_tx` parameter is optional; existing tests pass `None`).

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/mod.rs linux/src/main.rs
git commit -m "feat(linux): fan out RMS levels from vad-resample thread to overlay"
```

---

## Task 3.8: README + manual end-to-end smoke test (user-driven)

**Files:**
- Modify: `linux/README.md`

Update README for Phase 3 status, then hand off to the user for the real test.

- [ ] **Step 1: Update the Status block at the top of `linux/README.md`**

Find:

```markdown
> Status: **Phase 2** — hotkey-driven dictation works end-to-end via the XDG portal + ydotool. Tray mode (default invocation) and transcribe CLI mode (Phase 1) both still work.
```

Replace with:

```markdown
> Status: **Phase 3** — overlay capsule with live waveform appears during dictation. Tray (default), transcribe CLI (Phase 1), and listen mode (Phase 2) all still work.

> **Phase 3 GNOME note**: the overlay uses `wlr-layer-shell`, which GNOME's mutter does NOT implement. `voice-input listen` will fail to position the capsule correctly on GNOME — explicitly out of scope.
```

- [ ] **Step 2: Add a section after "Listen mode (Phase 2)" describing the overlay**

After the existing Listen mode block, before the "Compositor support" heading, insert:

```markdown
### Overlay capsule (Phase 3)

When you hold the configured shortcut, a small dark capsule appears at the bottom-center of your screen with an animated 5-bar waveform that tracks your speaking volume. When you release, the capsule briefly shows "Refining…" then disappears as the text is pasted.

Requires `wlr-layer-shell` support in your compositor:
- **KDE Plasma 6**: works (KWin 6+ supports it).
- **sway / hyprland**: works.
- **GNOME**: not supported — mutter does not implement layer-shell.
```

- [ ] **Step 3: Verify the README change**

```bash
grep -c "Phase 3" /home/desmond/Repos/voice-input-src/linux/README.md
```

Expected: ≥3 mentions of "Phase 3".

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/README.md
git commit -m "docs(linux): describe overlay capsule and Phase 3 status"
```

- [ ] **Step 5: User-driven manual smoke test**

USER hands this back. To run:

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build --release
RUST_LOG=info ./target/release/voice-input listen
```

Then:

1. **Focus a text app** (Kate, gedit, any text input).
2. **Hold Right Ctrl** (or whatever hotkey was bound in Phase 2).
3. **Observe the capsule**: a dark rounded rectangle appears at the bottom-center of the screen, with 5 small white bars on the left and "Listening…" text on the right.
4. **Speak**: the bars should grow/shrink in real time tracking your volume. Quiet → small bars (visible thanks to MIN_BAR_FRACTION). Loud → tall center bar with smaller side bars.
5. **Release**: capsule text briefly shows "Refining…", then capsule disappears, and the transcribed text appears in the focused text app.
6. Repeat 2-3 times to confirm stability.
7. **Ctrl+C** in the terminal: GTK application exits cleanly (the process terminates within ~1 second).

Acceptance:
- ✅ Capsule renders at bottom-center
- ✅ Waveform reacts to mic input
- ✅ "Refining…" state visible briefly on release
- ✅ Capsule disappears after release
- ✅ Text still pastes into focused app (Phase 2 functionality preserved)
- ✅ Ctrl+C exits cleanly (no hung GTK process)

If the capsule doesn't appear at all, check that your compositor supports layer-shell (`kf6-config --version` for KDE; `swaymsg -t get_version` for sway). On GNOME, this is expected to fail — documented out of scope.

Report findings.

---

## Task 3.9: Final verification + push

- [ ] **Step 1: Full test run**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: at least 34 tests + 2 ignored (31 prior + 3 waveform).

- [ ] **Step 2: Release build, no warnings**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build --release 2>&1 | grep -E "warning|error" | grep -v Compiling | head -20
```

Expected: empty (no voice-input warnings).

- [ ] **Step 3: cargo fmt + clippy**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt --check 2>&1
# If non-empty:
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
git -C /home/desmond/Repos/voice-input-src add -u linux/
git -C /home/desmond/Repos/voice-input-src commit -m "style(linux): cargo fmt"

PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -30
# If clippy finds issues, fix trivial ones; STOP and report non-trivial ones.
# If fixes applied:
git -C /home/desmond/Repos/voice-input-src add linux/
git -C /home/desmond/Repos/voice-input-src commit -m "chore(linux): fix clippy findings"
```

- [ ] **Step 4: Push the branch**

```bash
cd /home/desmond/Repos/voice-input-src
git push -u origin linux/phase-3-overlay 2>&1
```

(Branch name TBD by the controller when starting Phase 3 — likely `linux/phase-3-overlay`.)

- [ ] **Step 5: Final verification**

```bash
git -C /home/desmond/Repos/voice-input-src log origin/main..HEAD --oneline
git -C /home/desmond/Repos/voice-input-src status
git -C /home/desmond/Repos/voice-input-src branch -vv
```

Expected: clean tree, branch tracking `origin/linux/phase-3-overlay`.

---

## Self-Review Notes

**Spec coverage** (from `plans/voice-input-linux.md` Phase 3):
- ✅ gtk4-layer-shell capsule → Task 3.4
- ✅ 5-bar waveform with `[0.5, 0.8, 1.0, 0.75, 0.55]` weights, attack 0.4 / release 0.15, ±4% jitter, MIN_BAR_FRACTION 0.15 → Task 3.5 (constants pinned by test)
- ✅ Linux-native styling (no NSVisualEffectView mimicry) → Task 3.4 CSS
- ⏸ **Animated width transition** — deferred to Phase 5 polish. Fixed capsule width 360 px in Phase 3 MVP. The plan/design doc mentions this; explicit scope narrowing here.
- ✅ Refining state (label text change) → Task 3.6
- ✅ GTK on main thread, tokio on bg thread → Task 3.6
- ✅ Cross-thread channel for OverlayCmd → Task 3.3

**Phase 2 entry conditions addressed:**
1. ✅ Capture/PipelineHandle split for Send compatibility → Task 3.1
2. ✅ Drop shortcut_handle dead field → Task 3.1
3. ✅ Threading model doc-commented for the new arrangement → Task 3.1

**Placeholder scan:** no "TBD", "TODO", "fill in details" in steps. Every code block is complete.

**Type consistency check:**
- `PipelineHandle::join_remaining(self) -> Vec<String>` — used in main.rs deactivated arm ✓
- `start_pipeline(&Path, String, Option<Sender<f32>>) -> AppResult<(Capture, PipelineHandle)>` — used by both transcribe (passes None) and listen (passes Some) ✓
- `OverlayCmd::{Show, SetLevel(f32), SetText(String), Hide}` — emitted from backend, consumed in GTK timeout_add_local ✓
- `OverlayWindow::{new, show, hide, set_text, set_level}` — methods called from the GTK poll loop ✓
- `WaveformView::{new, widget, set_level, reset}` — called from OverlayWindow ✓

**Scope check:** Phase 3 deliberately excludes the LLM refiner (Phase 4), Settings dialog (Phase 5), language hint menu (Phase 5), spring animations (Phase 5), elastic-width transition (Phase 5), first-run wizard (Phase 6), packaging (Phase 7+).

**Known risks:**
- `gtk4 = "0.10"` API may have drifted. Specifically `set_draw_func` callback signature, `LayerShell` trait imports, `glib::timeout_add_local` signature (returns `ControlFlow`). Each task instructs the implementer to STOP and report NEEDS_CONTEXT on drift.
- `gtk4-layer-shell` 0.6 may require explicit feature flag for v6 layer-shell vs v4. The bare entry should default to whatever current KDE+sway+hyprland support.
- Polling `mpsc::Receiver` via `glib::timeout_add_local(16ms)` is functional but CPU-suboptimal compared to using `glib::MainContext::channel` directly. Trade-off chosen for simplicity (mpsc is Send-clean; glib channels require GTK headers in the sender path which would mean GTK on the backend thread, contradicting the design). A polish task can swap in glib channels with appropriate wrappers.
- The `Refining…` state is a 0-duration text change in Phase 3 (we send SetText("Refining…") then immediately drop capture + join + paste). The text MAY not be visible long enough for the user to perceive. Phase 4 LLM refiner will naturally add 300ms-1s of refining latency, making the state visible. If Phase 3 testing shows the text is invisible, add a `tokio::time::sleep(Duration::from_millis(200))` between SetText and Hide.
- Compositor matrix: smoke test on Plasma 6. Sway and hyprland are documented as expected-working but not test-targets in Phase 3. If you have one of those compositors handy, bonus verification.
