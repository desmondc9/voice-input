# Phase 7: Latency Polish + Autostart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Shave the remaining latency from "hotkey release → text pasted" by tuning the VAD silence cutoff and reusing both the whisper inference state and the Silero ONNX session across dictations, plus ship an autostart `.desktop` entry so the app starts at login.

**Architecture:** Item 1 is a single constant change in `vad.rs`. Items 2 and 3 share a pattern: introduce persistent worker threads that own the heavy resource (whisper `WhisperState` / Silero `VoiceActivityDetector`), and run a "session" per dictation driven by per-session crossbeam channels. The persistent threads sit idle between sessions awaiting a command. Item 4 is a small generator script that writes `~/.config/autostart/voice-input.desktop`.

**Tech Stack:** crossbeam-channel (existing), whisper-rs 0.14 with CUDA (existing), voice_activity_detector 0.2 (existing, exposes `reset()` to clear LSTM state).

---

## Pre-flight: Phase 6 entry conditions

Verify before starting:

- `main` at `e2ddc83` (Phase 6 + perf + docs merged)
- 52 tests pass, clippy clean
- CUDA build works: `ldd target/release/voice-input` shows `libcublas`
- User-side config: `whisper_model_size = "large-v3-turbo"`, `language_hint = "zh"` (or "")

Branch:

```bash
cd /home/desmond/Repos/voice-input-src
git checkout main
git pull --ff-only
git checkout -b linux/phase-7-latency-polish-autostart
```

---

## File structure

```
linux/
  src/
    speech/
      vad.rs              # MODIFY: SILENCE_WINDOWS_TO_CLOSE 10 → 5 (300→150ms)
                          # ADD:    optional VadSlicer::with_detector to inject Arc<Mutex<VoiceActivityDetector>>
      worker.rs           # ADD:    PersistentWhisperWorker (owns WhisperState across sessions)
      mod.rs              # MODIFY: start_pipeline accepts the two persistent workers, spawns short-lived adapter threads
    main.rs               # MODIFY: build PersistentWhisperWorker + PersistentVadWorker once in run_backend_async, pass refs per dictation
  scripts/
    install-autostart.sh  # NEW:    user-runnable script that writes ~/.config/autostart/voice-input.desktop
  README.md               # UPDATE: Phase 7 status, autostart section
```

No new dependencies.

---

## Task 7.1: VAD silence cutoff 300 ms → 150 ms

**Files:**
- Modify: `linux/src/speech/vad.rs`

The VAD waits for `SILENCE_WINDOWS_TO_CLOSE` consecutive silent windows (each 32 ms) before emitting a segment. Currently 10 windows ≈ 320 ms. Halving to 5 windows ≈ 160 ms makes the post-release pause feel noticeably snappier; the tradeoff is more segment splits within natural mid-sentence pauses, which is fine for dictation use.

- [ ] **Step 1: Locate the constant**

```bash
grep -n "SILENCE_WINDOWS_TO_CLOSE" linux/src/speech/vad.rs
```

Expected: a `const SILENCE_WINDOWS_TO_CLOSE: usize = 10;` line near the top, plus references inside `push` / `flush`.

- [ ] **Step 2: Edit the constant + the doc comment**

Replace:

```rust
/// How many trailing-silence windows close a segment (≥300 ms).
/// 300 ms / 32 ms ≈ 10 windows.
const SILENCE_WINDOWS_TO_CLOSE: usize = 10;
```

with:

```rust
/// How many trailing-silence windows close a segment (≈150 ms).
/// 150 ms / 32 ms ≈ 5 windows. Tuned in Phase 7 from 10 (≈300 ms) to
/// reduce the post-release pause; the cost is more aggressive
/// mid-sentence splits, which is acceptable for hold-to-talk dictation
/// where the speaker is expected to talk in short utterances.
const SILENCE_WINDOWS_TO_CLOSE: usize = 5;
```

- [ ] **Step 3: Build + tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo test --lib vad 2>&1 | tail -10
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: build clean, vad tests pass, clippy clean. If a vad test pinned the old 300 ms / 10-window count, update its expected values to match the new constant.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/vad.rs
git commit -m "perf(linux): VAD silence cutoff 300 ms → 150 ms for snappier release"
```

---

## Task 7.2: Persistent whisper worker (reuse `WhisperState`)

**Files:**
- Modify: `linux/src/speech/worker.rs`
- Modify: `linux/src/speech/mod.rs`
- Modify: `linux/src/main.rs`

Currently `worker::spawn` is called per dictation; inside it `ctx.create_state()` allocates ~280 MB of KV-cache + compute buffers (logged as `whisper_init_state: ...` lines). Lift `WhisperState` to a long-lived `PersistentWhisperWorker` thread that sits idle between sessions and runs inference when a `Run` command arrives.

- [ ] **Step 1: Add the `PersistentWhisperWorker` type in `worker.rs`**

Append to `linux/src/speech/worker.rs` (do NOT remove `load_whisper_context`; it's still used to build the `Arc<WhisperContext>`).

```rust
/// Long-lived whisper worker that owns a `WhisperState` across many
/// dictations. Built once at app startup; each dictation calls
/// `start_session` with fresh per-dictation channels. The worker thread
/// itself never exits until the user issues `shutdown` (or the cmd
/// channel is closed).
pub struct PersistentWhisperWorker {
    cmd_tx: crossbeam_channel::Sender<WorkerCmd>,
    handle: Option<thread::JoinHandle<()>>,
}

enum WorkerCmd {
    Run {
        language_hint: String,
        slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
        text_tx: crossbeam_channel::Sender<String>,
    },
    Shutdown,
}

impl PersistentWhisperWorker {
    pub fn spawn(ctx: Arc<WhisperContext>) -> AppResult<Self> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<WorkerCmd>(1);
        let handle = thread::Builder::new()
            .name("whisper-worker-persistent".into())
            .spawn(move || run_persistent(ctx, cmd_rx))
            .map_err(|e| AppError::WhisperFailed(format!("spawn persistent worker: {}", e)))?;
        Ok(Self {
            cmd_tx,
            handle: Some(handle),
        })
    }

    /// Run one dictation's worth of inference. Returns after the
    /// command is enqueued; the caller signals end-of-session by
    /// dropping the corresponding `slice_tx`. When `text_tx` is dropped
    /// by the worker (after slice_rx closes), the caller's `text_rx`
    /// will see Disconnected.
    pub fn start_session(
        &self,
        language_hint: String,
        slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
        text_tx: crossbeam_channel::Sender<String>,
    ) -> AppResult<()> {
        self.cmd_tx
            .send(WorkerCmd::Run {
                language_hint,
                slice_rx,
                text_tx,
            })
            .map_err(|e| AppError::WhisperFailed(format!("send Run: {}", e)))
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PersistentWhisperWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_persistent(ctx: Arc<WhisperContext>, cmd_rx: crossbeam_channel::Receiver<WorkerCmd>) {
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "persistent whisper worker: create_state failed");
            return;
        }
    };
    tracing::info!("persistent whisper worker: state ready");

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCmd::Run {
                language_hint,
                slice_rx,
                text_tx,
            } => {
                run_session(&mut state, &language_hint, slice_rx, text_tx);
            }
            WorkerCmd::Shutdown => break,
        }
    }
    tracing::info!("persistent whisper worker: exiting");
}

fn run_session(
    state: &mut whisper_rs::WhisperState,
    language_hint: &str,
    slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
    text_tx: crossbeam_channel::Sender<String>,
) {
    while let Ok(slice) = slice_rx.recv() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if !language_hint.is_empty() {
            params.set_language(Some(language_hint));
        }
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        if let Err(e) = state.full(params, &slice) {
            tracing::warn!(error = %e, samples = slice.len(), "whisper inference failed; skipping slice");
            continue;
        }

        let n_segments = match state.full_n_segments() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "full_n_segments failed");
                continue;
            }
        };
        let mut combined = String::new();
        for i in 0..n_segments {
            match state.full_get_segment_text(i) {
                Ok(text) => {
                    if !combined.is_empty() {
                        combined.push(' ');
                    }
                    combined.push_str(text.trim());
                }
                Err(e) => tracing::warn!(error = %e, segment = i, "get_segment_text failed"),
            }
        }
        let trimmed = combined.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if text_tx.send(trimmed).is_err() {
            tracing::info!("whisper session: text channel closed, exiting session");
            return;
        }
    }
    tracing::info!("whisper session: slice channel closed, session done");
}
```

Keep the OLD `spawn(ctx, language_hint, slice_rx, text_tx)` function and its `fn run(...)` for now — they're unused after this task but removing them touches mod.rs's tests; we'll delete them in Step 4 below once mod.rs is migrated.

- [ ] **Step 2: Update `start_pipeline` in `speech/mod.rs`**

Replace the existing `start_pipeline` body. New signature accepts a `&PersistentWhisperWorker` instead of building a worker per call:

```rust
pub fn start_pipeline(
    whisper_worker: &worker::PersistentWhisperWorker,
    language_hint: String,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
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
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels, level_tx);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;

    // Hand the slice receiver + text sender to the persistent whisper
    // worker. It will keep running across many dictations; we just give
    // it new per-session channels each time.
    whisper_worker.start_session(language_hint, slice_rx, text_tx)?;

    Ok((
        capture,
        PipelineHandle {
            text_rx,
            vad_handle: Some(vad_handle),
        },
    ))
}
```

Update `PipelineHandle` to no longer hold a whisper handle (since the worker is persistent now). Replace:

```rust
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    vad_handle: Option<JoinHandle<()>>,
    whisper_handle: Option<JoinHandle<()>>,
}
```

with:

```rust
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    vad_handle: Option<JoinHandle<()>>,
}
```

Update `join_remaining` and the `Drop` impl correspondingly — remove the `whisper_handle` join calls, but the body of `join_remaining` now needs to drain text_rx until disconnected (the worker will drop text_tx after `slice_rx` closes, which closes when vad_handle joins). New `join_remaining`:

```rust
    pub fn join_remaining(mut self) -> Vec<String> {
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        // VAD has dropped slice_tx by now → whisper worker's session
        // loop exits → whisper drops text_tx → recv loop terminates.
        let mut out = Vec::new();
        while let Ok(seg) = self.text_rx.recv() {
            out.push(seg);
        }
        out
    }
```

And in `Drop`:

```rust
impl Drop for PipelineHandle {
    fn drop(&mut self) {
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
    }
}
```

- [ ] **Step 3: Update `main.rs` call sites**

In `run_transcribe`, replace:

```rust
    let whisper_ctx = std::sync::Arc::new(
        voice_input::speech::worker::load_whisper_context(&model_path)
            .context("loading whisper model")?,
    );
    tracing::info!("whisper model loaded");

    let (_capture, pipeline) =
        voice_input::speech::start_pipeline(whisper_ctx, cfg.language_hint.clone(), None)
            .context("starting speech pipeline")?;
```

with:

```rust
    let whisper_ctx = std::sync::Arc::new(
        voice_input::speech::worker::load_whisper_context(&model_path)
            .context("loading whisper model")?,
    );
    tracing::info!("whisper model loaded");

    let whisper_worker = voice_input::speech::worker::PersistentWhisperWorker::spawn(whisper_ctx)
        .context("spawning persistent whisper worker")?;

    let (_capture, pipeline) =
        voice_input::speech::start_pipeline(&whisper_worker, cfg.language_hint.clone(), None)
            .context("starting speech pipeline")?;
```

In `run_backend_async`, the corresponding section just after `tracing::info!("whisper model loaded");` should add the worker spawn:

```rust
    let whisper_ctx = std::sync::Arc::new(
        speech::worker::load_whisper_context(&model_path).context("loading whisper model")?,
    );
    tracing::info!("whisper model loaded");

    let whisper_worker = speech::worker::PersistentWhisperWorker::spawn(whisper_ctx)
        .context("spawning persistent whisper worker")?;
```

And the per-dictation `start_pipeline` call in the Activated arm changes from:

```rust
                match speech::start_pipeline(std::sync::Arc::clone(&whisper_ctx), snap.language_hint.clone(), Some(level_tx.clone())) {
```

to:

```rust
                match speech::start_pipeline(&whisper_worker, snap.language_hint.clone(), Some(level_tx.clone())) {
```

Note: `whisper_ctx` is no longer used directly after the worker is built — drop the binding name or hold it just for documentation. Cleanest is to inline: `PersistentWhisperWorker::spawn(Arc::new(load_whisper_context(&model_path)?))`. Either is fine; pick the form that keeps the diff small.

- [ ] **Step 4: Delete the old `worker::spawn` + `fn run` now that nothing uses them**

In `linux/src/speech/worker.rs`, delete the old `pub fn spawn(ctx, language_hint, slice_rx, text_tx)` function and the old non-persistent `fn run(...)`. Leave `load_whisper_context` in place. Also delete the now-orphaned `missing_model_path_returns_model_missing_error` and `transcribes_silence_to_empty_or_short_text` only if they still compile against the new API; otherwise update them. Goal: zero dead code.

For the `missing_model_path_returns_model_missing_error` test it already only depends on `load_whisper_context` — keep it as-is.

The ignored real-inference test should be updated to use the persistent worker:

```rust
    #[test]
    #[ignore]
    fn transcribes_silence_to_empty_or_short_text() {
        let model_path = std::env::var("VOICE_INPUT_MODEL_PATH")
            .or_else(|_| {
                Ok::<_, std::env::VarError>(
                    dirs_for_test()
                        .join("ggml-tiny.bin")
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .unwrap();
        let path = PathBuf::from(&model_path);
        if !path.exists() {
            eprintln!("skipping: model not at {}", model_path);
            return;
        }
        let ctx = Arc::new(load_whisper_context(&path).unwrap());
        let mut worker = PersistentWhisperWorker::spawn(ctx).unwrap();
        let (slices_tx, slices_rx) = crossbeam_channel::bounded(1);
        let (text_tx, text_rx) = crossbeam_channel::bounded(1);
        worker.start_session("en".into(), slices_rx, text_tx).unwrap();

        let silence = vec![0.0_f32; 16_000 * 3];
        slices_tx.send(silence).unwrap();
        drop(slices_tx);

        let _ = text_rx.recv_timeout(std::time::Duration::from_secs(30));
        worker.shutdown();
    }
```

- [ ] **Step 5: Build + tests + clippy + release build to verify CUDA still links**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo test 2>&1 | grep "test result"
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build --release 2>&1 | tail -5
```

All clean. Test count should match the previous total (no new lib tests added, ignored one updated).

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/worker.rs linux/src/speech/mod.rs linux/src/main.rs
git commit -m "perf(linux): persistent whisper worker - reuse WhisperState across dictations"
```

---

## Task 7.3: Persistent Silero VAD (hoist ONNX session)

**Files:**
- Modify: `linux/src/speech/vad.rs`
- Modify: `linux/src/speech/mod.rs` (where `VadSlicer::new` is called inside `run_vad_resample`)

`VoiceActivityDetector` from `voice_activity_detector 0.2.1` exposes `reset(&mut self)` which clears its LSTM hidden state — perfect for reuse across dictations. The session-creation cost (ONNX graph build + initializer pruning) is what we're saving.

The simplest pattern: wrap a single `VoiceActivityDetector` in `Arc<Mutex<...>>` shared by main.rs; per dictation, the per-dictation `VadSlicer` borrows it through the mutex. Since only one dictation can be active at a time, contention is impossible.

- [ ] **Step 1: Refactor `VadSlicer` to take an external detector**

In `linux/src/speech/vad.rs`, change `VadSlicer` to hold a reference instead of owning the detector. Replace:

```rust
pub struct VadSlicer {
    vad: VoiceActivityDetector,
    window_buf: Vec<f32>,
    segment: Vec<f32>,
    silence_count: usize,
    in_segment: bool,
}

impl VadSlicer {
    pub fn new() -> AppResult<Self> {
        let vad = VoiceActivityDetector::builder()
            .sample_rate(VAD_SAMPLE_RATE as i64)
            .chunk_size(VAD_WINDOW)
            .build()
            .map_err(|e| AppError::WhisperFailed(format!("vad init: {}", e)))?;
        Ok(Self {
            vad,
            window_buf: Vec::with_capacity(VAD_WINDOW),
            segment: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            silence_count: 0,
            in_segment: false,
        })
    }
```

with:

```rust
pub struct VadSlicer {
    /// Heavy ONNX-backed detector, shared across dictations and reset
    /// between them. Held under a Mutex because Silero's predict() is
    /// `&mut self` (LSTM state mutates each call) but only one dictation
    /// is active at a time, so contention is impossible.
    detector: std::sync::Arc<std::sync::Mutex<VoiceActivityDetector>>,
    window_buf: Vec<f32>,
    segment: Vec<f32>,
    silence_count: usize,
    in_segment: bool,
}

impl VadSlicer {
    /// Build a brand-new detector. Use this once at app startup and
    /// share via Arc.
    pub fn build_detector() -> AppResult<VoiceActivityDetector> {
        VoiceActivityDetector::builder()
            .sample_rate(VAD_SAMPLE_RATE as i64)
            .chunk_size(VAD_WINDOW)
            .build()
            .map_err(|e| AppError::WhisperFailed(format!("vad init: {}", e)))
    }

    /// Create a per-dictation slicer that shares the given detector.
    /// Resets the detector's LSTM state so this dictation starts clean.
    pub fn new_with_detector(
        detector: std::sync::Arc<std::sync::Mutex<VoiceActivityDetector>>,
    ) -> Self {
        detector.lock().unwrap().reset();
        Self {
            detector,
            window_buf: Vec::with_capacity(VAD_WINDOW),
            segment: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            silence_count: 0,
            in_segment: false,
        }
    }
```

Update every `self.vad.predict(...)` inside the existing `push` method to `self.detector.lock().unwrap().predict(...)`. This is the same single call replacement applied wherever it appears.

If there's a `VadSlicer::new()` still used by tests, replace test setups with a fresh `Arc<Mutex<...>>` built from `VadSlicer::build_detector()` and `new_with_detector` to wrap it.

- [ ] **Step 2: Plumb the shared detector through `start_pipeline`**

In `linux/src/speech/mod.rs`, change `start_pipeline` to accept the shared detector and pass it into the VAD-resample thread. New signature:

```rust
pub fn start_pipeline(
    whisper_worker: &worker::PersistentWhisperWorker,
    vad_detector: std::sync::Arc<std::sync::Mutex<voice_activity_detector::VoiceActivityDetector>>,
    language_hint: String,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
) -> AppResult<(Capture, PipelineHandle)> {
    // ... same as before, but in run_vad_resample, build the slicer via
    // VadSlicer::new_with_detector(vad_detector) instead of VadSlicer::new()
}
```

Inside `run_vad_resample`, replace the `let mut slicer = match vad::VadSlicer::new() { ... }` block with `let mut slicer = vad::VadSlicer::new_with_detector(vad_detector);` (no Result, since the detector is pre-built; reset is infallible).

- [ ] **Step 3: Update `main.rs` callers**

In `run_backend_async`, after `whisper_worker` is built, add:

```rust
    let vad_detector = std::sync::Arc::new(std::sync::Mutex::new(
        voice_input::speech::vad::VadSlicer::build_detector()
            .context("building shared VAD detector")?,
    ));
    tracing::info!("vad detector ready");
```

And the per-dictation call becomes:

```rust
                match speech::start_pipeline(&whisper_worker, std::sync::Arc::clone(&vad_detector), snap.language_hint.clone(), Some(level_tx.clone())) {
```

`run_transcribe` similarly: build the detector once and pass into the single `start_pipeline` call.

- [ ] **Step 4: Update VAD unit tests in `vad.rs`**

If existing tests call `VadSlicer::new()`, update them to:

```rust
let detector = std::sync::Arc::new(std::sync::Mutex::new(
    VadSlicer::build_detector().unwrap(),
));
let mut slicer = VadSlicer::new_with_detector(detector);
```

- [ ] **Step 5: Build + tests + clippy + release build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo test 2>&1 | grep "test result"
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build --release 2>&1 | tail -5
```

All clean.

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/vad.rs linux/src/speech/mod.rs linux/src/main.rs
git commit -m "perf(linux): hoist Silero VAD - share VoiceActivityDetector across dictations with reset()"
```

---

## Task 7.4: Autostart `.desktop` install script

**Files:**
- Create: `linux/scripts/install-autostart.sh`

A small helper that writes a `~/.config/autostart/voice-input.desktop` entry pointing at the user's installed binary. No code changes; this is a user-facing utility.

- [ ] **Step 1: Create the script**

Create `linux/scripts/install-autostart.sh`:

```bash
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
```

Make it executable:

```bash
chmod +x /home/desmond/Repos/voice-input-src/linux/scripts/install-autostart.sh
```

- [ ] **Step 2: Sanity-check the generator**

Run it once to confirm it actually produces a valid file (no need to actually install):

```bash
cd /home/desmond/Repos/voice-input-src/linux
./scripts/install-autostart.sh ./target/release/voice-input
cat ~/.config/autostart/voice-input.desktop
desktop-file-validate ~/.config/autostart/voice-input.desktop 2>&1 || echo "(desktop-file-validate not installed; manual check)"
```

Expected: file at `~/.config/autostart/voice-input.desktop` containing the `[Desktop Entry]` block. `desktop-file-validate` (if installed) reports no errors.

- [ ] **Step 3: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/scripts/install-autostart.sh
git commit -m "feat(linux): install-autostart.sh - generate XDG autostart .desktop entry"
```

---

## Task 7.5: README + smoke test + push + merge

**Files:**
- Modify: `linux/README.md`

- [ ] **Step 1: Update README**

Edit line 5 (Status block). Replace:

```markdown
> Status: **Phase 6** — tray icon now reflects pipeline state (`media-record-symbolic` while dictating, `audio-input-microphone` idle), matching the macOS `mic`/`mic.fill` parity. Phase 5 features (Settings dialog, Enabled / Language / LLM Refinement submenus, unified default mode) remain. Headless `transcribe` CLI still works.
```

with:

```markdown
> Status: **Phase 7** — latency polish (persistent whisper + VAD workers, VAD silence cutoff 150 ms) plus XDG autostart installer. Phase 6 recording icon, Phase 5 tray menus, headless `transcribe` CLI all unchanged.
```

Append a new section after the LLM-refinement section:

```markdown
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
```

- [ ] **Step 2: cargo fmt + final test + clippy sanity**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo fmt
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo fmt -- --check 2>&1 | head -5
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo test 2>&1 | grep "test result"
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
PATH="$HOME/.cargo/bin:/usr/local/cuda/bin:$PATH" cargo build --release 2>&1 | tail -3
```

All clean.

- [ ] **Step 3: User smoke test**

```bash
cd /home/desmond/Repos/voice-input-src/linux
RUST_LOG=info ./target/release/voice-input
```

Acceptance checklist (have a human watch):

1. Startup logs include both `persistent whisper worker: state ready` (once) AND `vad detector ready` (once). No further `whisper_init_state` or ONNX `Session Options` logs appear on subsequent hotkey presses.
2. First dictation cycle: press → record → release. End-to-end "release → text pasted" should feel ≤ ~300 ms for a short utterance with `small` model; ~400 ms with `large-v3-turbo` (down from ~600 ms in Phase 6).
3. Second and third dictation cycles complete with NO additional ONNX init logs and NO additional whisper_init_state logs.
4. Tray icon transitions still work (red while holding, idle the moment release).
5. Toggling Enabled / Language / LLM Refinement / Settings dialog all still work.
6. Quit from tray exits in ≤ 1 s.

- [ ] **Step 4: Run autostart installer + verify**

```bash
cd /home/desmond/Repos/voice-input-src
./linux/scripts/install-autostart.sh ./linux/target/release/voice-input
cat ~/.config/autostart/voice-input.desktop
```

Confirm file is present and well-formed. Optional: log out and back in to verify it starts automatically.

- [ ] **Step 5: Push + merge**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/README.md
git commit -m "docs(linux): describe Phase 7 latency polish + autostart"
git push -u origin linux/phase-7-latency-polish-autostart

git checkout main
git merge --ff-only linux/phase-7-latency-polish-autostart
git push origin main
```

---

## Self-Review Notes

**Spec coverage:**
- VAD silence 300→150 ms → Task 7.1
- WhisperState reuse → Task 7.2 (PersistentWhisperWorker)
- Silero VAD reuse → Task 7.3 (Arc<Mutex<VoiceActivityDetector>> + reset)
- Autostart .desktop → Task 7.4 (install script)
- All four bundled docs → Task 7.5

**Architectural decisions:**
- Persistent-thread pattern for whisper: cleaner than transferring `WhisperState` through channels because `WhisperState` is `!Sync` and the thread-affinity matters; pinning it to a single long-lived thread keeps borrow rules trivial.
- Arc<Mutex> for VAD: simpler than spawning a persistent VAD thread because `VoiceActivityDetector::predict` is `&mut self` and the per-dictation slicer state (`window_buf`, etc.) doesn't need to live in the same thread. Single-writer mutex contention is zero by construction.
- `reset()` called inside `VadSlicer::new_with_detector` ensures clean LSTM state at the start of every dictation.
- Autostart as a script rather than a vendored static `.desktop` file: the user's binary path varies (may be `~/.cargo/bin/voice-input`, `/usr/local/bin/voice-input`, or repo-local `target/release/`); the generator picks correctly.

**Known risks:**
- VAD silence 150 ms could cut mid-sentence on slow speakers. Acceptable for hold-to-talk where speakers pause briefly between sentences; if it bites, easy tweak back to 8 or 10 windows.
- The persistent whisper worker holds `WhisperState` forever. If RAM pressure becomes a problem we could add a hibernate mode, but a single state at ~280 MB is fine even on laptops.
- ksni `Handle::update` is unaffected — Phase 6 wiring continues to work as the tray reads state.recording on every D-Bus query.
- The persistent worker's `text_tx` drop is what signals end-of-session to the GTK side. If the worker thread panics mid-session, `text_tx` is still dropped by Rust's panic unwind, so the consumer doesn't hang. The thread itself dies, leaving the persistent worker in a dead state — subsequent dictations would fail at `start_session` (channel closed). Acceptable for now; if it bites, add a respawn-on-panic supervisor.

**Out of scope (deferred):**
- GPU-accelerated VAD (Silero on CUDA via ONNX Runtime GPU EP) — bigger refactor.
- Audio capture device reuse (currently fresh `cpal::Stream` per dictation) — `cpal::Stream` is `!Send` and tied to the GTK/main thread; reusing is invasive.
- Tray icon "refining" tristate (idle / recording / refining) — Phase 6.5 candidate.
