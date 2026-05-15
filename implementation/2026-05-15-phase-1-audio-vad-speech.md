# Phase 1 — Audio + VAD + Speech Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a CLI-only audio→VAD→whisper pipeline that captures from the default microphone, slices on speech pauses, transcribes each slice with whisper.cpp, and prints segments to stdout. The existing Phase 0 tray binary continues to work unchanged via the default (no-subcommand) invocation.

**Architecture:** Three internal modules wired by a thin pipeline starter. `audio.rs` opens cpal at the device's native rate, computes per-buffer RMS, and resamples to whisper's required 16 kHz mono f32. `speech/vad.rs` slices the 16 kHz stream into segments using Silero VAD. `speech/worker.rs` runs whisper-rs on a dedicated `std::thread` (whisper state is `!Send`) and emits transcripts on a channel. `speech/mod.rs` exposes `start_pipeline()` which wires everything. `main.rs` gains a clap-based subcommand split: no args = tray (Phase 0); `transcribe` = run the pipeline and print segments. **All worker threads use `std::thread` + `crossbeam_channel`, NOT tokio tasks** — this keeps the pipeline runtime-agnostic so Phase 3's GTK4 main-thread rework doesn't have to rewrite it.

**Tech Stack:**
- `cpal = "0.15"` (audio capture, ALSA/PulseAudio/PipeWire under the hood)
- `rubato = "0.16"` (high-quality sample-rate conversion to 16 kHz)
- `voice_activity_detector = "0.2"` (Silero VAD via ONNX runtime; bundled model)
- `whisper-rs = "0.14"` (whisper.cpp Rust bindings; needs libclang + cmake at build time)
- `crossbeam-channel = "0.5"` (bounded MPSC, simpler than tokio channels for sync workers)
- `clap = { version = "4", features = ["derive"] }` (CLI subcommands)
- `hound = "3"` (WAV file writing — used by debug `--dump-audio` flag for offline debugging)

**Reference spec:** `plans/voice-input-linux.md` — Phase 1 section + module port map rows for `audio.rs`, `speech.rs`, plus the Wayland gotcha note about whisper VAD-sliced segments.

**System dependencies confirmed present (Phase 1 entry condition):** `cmake 4.2.3`, `libclang-21.so.21` at `/usr/lib/x86_64-linux-gnu/`, `cc/gcc 15`, `pkg-config 2.5.1`. If `bindgen` (used by whisper-rs build) fails to locate libclang, set `LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu/`.

**Build-time warning for engineer:** First `cargo build` after Task 1.1 will compile whisper.cpp from source (≈30–60s on first run, cached afterwards). This is normal.

---

## File Structure (after Phase 1)

| Path | Responsibility |
|---|---|
| `linux/Cargo.toml` | Add Phase 1 deps |
| `linux/src/lib.rs` | Add `pub mod audio; pub mod cli; pub mod speech;` |
| `linux/src/audio.rs` | cpal capture, RMS computation, rubato resampler |
| `linux/src/cli.rs` | clap subcommand parsing (`Cli` struct + `Command` enum) |
| `linux/src/speech/mod.rs` | Public `start_pipeline()` entry, re-exports |
| `linux/src/speech/vad.rs` | Silero VAD slicer (16 kHz mono f32 in, `Vec<f32>` slices out) |
| `linux/src/speech/worker.rs` | Whisper-rs worker thread (slices in, transcripts out) |
| `linux/src/main.rs` | Modified to route on subcommand (default tray, or `transcribe`) |
| `linux/src/config.rs` | Add helper `Config::resolve_model_path()` that resolves `whisper_model_path` or falls back to XDG data dir |
| `linux/README.md` | Updated build/run section + model download instructions |
| `linux/tests/audio_rms.rs` | Integration test for RMS normalization |
| `linux/tests/vad_slicing.rs` | Integration test for VAD slicer with synthetic audio |

**Files NOT touched in Phase 1:** `app.rs` (state machine stub — Phase 5 wires it), `tray.rs` (Phase 5 expands menu), `error.rs` (already covers the error kinds we need).

---

## Threading & data flow

```
main thread                          audio thread (cpal callback)
    │                                       │
    ▼                                       │ raw_chunks (any rate)
[clap parses CLI]                           │
    │                                       ▼
    ├── (no args) → Phase 0 tray flow  [audio::Capture]
    │                                       │
    └── (transcribe) ───────────────────────┤ rms (f32) ──→ stdout (info log)
                                            │
                                            ▼  16kHz mono f32
                                       [resample]
                                            │
                                            ▼ on bounded channel
                                       VAD thread (std::thread)
                                            │ slice (Vec<f32>) on bounded channel
                                            ▼
                                       Whisper worker (std::thread)
                                            │ String on channel
                                            ▼
                                       Pipeline driver (main thread)
                                            │
                                            ▼
                                       println!("[segment N] {text}")
```

`tokio` is no longer required for the CLI path (`transcribe` subcommand). The Phase 0 tray path still uses `#[tokio::main]`. We split: `main.rs` enters tokio runtime ONLY for the tray subcommand; the CLI transcribe path uses plain `std::thread`. This decouples the speech pipeline from tokio so Phase 3 doesn't have to rewrite it.

---

## Open design decisions resolved before tasks begin

1. **Sample rate strategy:** Always resample to 16 kHz mono f32 via rubato. Don't try to pick a 16 kHz-native cpal config — most consumer mics report 44.1/48 kHz default and forcing 16 kHz fails on USB conferencing mics. Resampling is cheap (~CPU% per channel).
2. **VAD threshold defaults:** `voice_activity_detector` 0.5 probability threshold, 30 ms chunks, ≥300 ms of trailing silence to close a segment, segment length capped at 30 s (matches whisper context window). Tunable via config later; hard-coded in Phase 1.
3. **Whisper model path resolution:** Config field `whisper_model_path` (if `Some`) wins; otherwise fall back to `~/.local/share/voice-input/models/ggml-{whisper_model_size}.bin`. Env var `VOICE_INPUT_MODEL_PATH` overrides both (for dev). If no model file found at the resolved path, emit `AppError::ModelMissing { path }` with the README's download command in the message.
4. **Corrupted config behavior** (Phase 1 entry condition #4 from the Phase 0 final review): crash with clear `AppError::Config` message. Silent fallback hides bugs.
5. **Whisper integration tests:** Default `cargo test` skips whisper inference (no model file in CI). Tag with `#[ignore]` and document `cargo test -- --ignored` for local runs.
6. **Threading model documentation for Phase 3** (Phase 1 entry condition #1): the pipeline modules in `audio.rs`, `speech/vad.rs`, `speech/worker.rs` are deliberately `std::thread` + `crossbeam_channel` based. Phase 3 will rewrite `main.rs` to run tokio on a background thread and GTK on the main thread; pipeline modules don't change. **Add a comment in `speech/mod.rs::start_pipeline` referencing this decision** so a future maintainer doesn't "modernize" it into tokio tasks.

---

## Task 1.1: Add Phase 1 dependencies + clap CLI skeleton

**Files:**
- Modify: `linux/Cargo.toml` (add deps)
- Create: `linux/src/cli.rs`
- Modify: `linux/src/lib.rs` (add `pub mod cli;`)
- Modify: `linux/src/main.rs` (dispatch on CLI subcommand; tray flow unchanged)

- [ ] **Step 1: Add dependencies to `linux/Cargo.toml`**

In the `[dependencies]` section, add (preserve existing entries, add these alphabetically):

```toml
clap = { version = "4", features = ["derive"] }
cpal = "0.15"
crossbeam-channel = "0.5"
hound = "3"
rubato = "0.16"
voice_activity_detector = "0.2"
whisper-rs = "0.14"
```

The full updated `[dependencies]` section in alphabetical order:

```toml
[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
cpal = "0.15"
crossbeam-channel = "0.5"
directories = "5"
hound = "3"
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
```

- [ ] **Step 2: Trigger first build to compile whisper-rs**

Run from `/home/desmond/Repos/voice-input-src/linux`:

```bash
cargo build 2>&1 | tail -10
```

Expected: a `Finished` line. **This first build will take 30–90 seconds** because whisper.cpp compiles from source. If you see `unable to find libclang`, run:

```bash
LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu/ cargo build
```

and report a build environment issue (the regular build should find libclang via ldconfig).

If a dep version is yanked or doesn't resolve (e.g. `whisper-rs 0.14` doesn't exist), STOP and report `NEEDS_CONTEXT` with the exact error.

- [ ] **Step 3: Add `pub mod cli;` to `linux/src/lib.rs`**

Modify `linux/src/lib.rs` to read (alphabetical, full content):

```rust
pub mod app;
pub mod cli;
pub mod config;
pub mod error;
pub mod tray;
```

- [ ] **Step 4: Create `linux/src/cli.rs`**

```rust
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "voice-input", version, about = "Wayland-native voice input")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the audio capture + whisper transcription pipeline and print
    /// segments to stdout. No tray, no UI. Press Ctrl+C to stop.
    Transcribe,
}
```

- [ ] **Step 5: Modify `linux/src/main.rs` to dispatch on subcommand**

Replace the entire file content with:

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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load().context("loading config")?;
    cfg.save().context("persisting config defaults")?;
    tracing::info!(
        language_hint = %cfg.language_hint,
        llm_enabled = cfg.llm_enabled,
        whisper_model_size = %cfg.whisper_model_size,
        "config loaded"
    );

    match cli.command {
        None => run_tray(cfg),
        Some(Command::Transcribe) => run_transcribe(cfg),
    }
}

fn run_tray(_cfg: Config) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(async {
        let shutdown = Arc::new(Notify::new());
        let tray = VoiceInputTray::new(shutdown.clone());
        let _tray_handle = tray.spawn().await.context("spawning tray")?;

        tracing::info!("voice-input running — Quit via tray icon or Ctrl+C");

        tokio::select! {
            _ = shutdown.notified() => tracing::info!("tray Quit received"),
            _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received"),
        }

        tracing::info!("shutdown complete");
        Ok::<(), anyhow::Error>(())
    })
}

fn run_transcribe(_cfg: Config) -> anyhow::Result<()> {
    // Implemented in Task 1.7. For now, just print a placeholder so the CLI
    // dispatch is testable end-to-end.
    println!("transcribe subcommand: pipeline wiring not yet implemented (Task 1.7)");
    Ok(())
}
```

Note: `#[tokio::main]` is gone — the tray flow now builds its own runtime explicitly, and the transcribe path is fully synchronous.

- [ ] **Step 6: Verify build and existing tests still pass**

Run from `/home/desmond/Repos/voice-input-src/linux`:

```bash
cargo build 2>&1 | tail -5
cargo test 2>&1 | grep "test result"
```

Expected: clean build; all 10 existing tests still pass (no regressions).

- [ ] **Step 7: Smoke-test CLI dispatch**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo run -- --help 2>&1 | head -15
cargo run -- transcribe 2>&1 | head -5
timeout 2 cargo run 2>&1 | head -5  # default = tray path; timeout kills it
```

Expected:
- `--help` shows the `transcribe` subcommand
- `transcribe` prints the placeholder line and exits
- default invocation prints `config loaded ... voice-input running — Quit via tray icon or Ctrl+C` then is killed by timeout

- [ ] **Step 8: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/Cargo.lock linux/src/cli.rs linux/src/lib.rs linux/src/main.rs
git commit -m "feat(linux): add Phase 1 dependencies and clap CLI dispatch"
```

(No `Co-Authored-By` trailer.)

---

## Task 1.2: RMS normalization (pure function, TDD)

**Files:**
- Create: `linux/src/audio.rs`
- Modify: `linux/src/lib.rs` (add `pub mod audio;`)
- Create: `linux/tests/audio_rms.rs`

The RMS normalization is the same formula as the macOS version (`dist/Sources/VoiceInput/SpeechEngine.swift:97-103`): compute RMS, take 20·log₁₀, map (-50 dB, -10 dB) to (0, 1) and clamp.

- [ ] **Step 1: Add `pub mod audio;` to `linux/src/lib.rs`**

Updated `linux/src/lib.rs`:

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod tray;
```

- [ ] **Step 2: Write `linux/src/audio.rs` with the pure RMS function**

```rust
/// Compute RMS over `samples` and normalize the resulting dBFS value to
/// the [0, 1] range using the same mapping as the macOS version:
/// `normalized = clamp((dB + 50) / 40, 0, 1)`.
/// dB is computed from `max(rms, 1e-6)` to avoid `log10(0)`.
pub fn rms_normalized(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt().max(1e-6);
    let db = 20.0 * rms.log10();
    ((db + 50.0) / 40.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_yields_zero() {
        let silence = vec![0.0_f32; 1024];
        let level = rms_normalized(&silence);
        assert!(level < 0.05, "expected near zero, got {}", level);
    }

    #[test]
    fn full_scale_sine_yields_one() {
        // Full-scale 1 kHz sine at 16 kHz sample rate
        let samples: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16_000.0).sin())
            .collect();
        let level = rms_normalized(&samples);
        assert!(level > 0.9, "expected near one, got {}", level);
    }

    #[test]
    fn empty_returns_zero() {
        assert_eq!(rms_normalized(&[]), 0.0);
    }

    #[test]
    fn quiet_noise_maps_to_low_range() {
        let quiet: Vec<f32> = (0..1024).map(|i| ((i as f32 * 0.1).sin()) * 0.001).collect();
        let level = rms_normalized(&quiet);
        assert!(level >= 0.0 && level < 0.2, "expected low range, got {}", level);
    }
}
```

- [ ] **Step 3: Write `linux/tests/audio_rms.rs` for integration-level coverage**

```rust
use voice_input::audio::rms_normalized;

#[test]
fn monotonic_increase_with_amplitude() {
    let amplitudes = [0.001_f32, 0.01, 0.05, 0.2, 0.7];
    let mut prev = -1.0_f32;
    for amp in amplitudes {
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32).sin() * amp).collect();
        let level = rms_normalized(&samples);
        assert!(
            level >= prev,
            "level for amplitude {} ({}) should be >= previous ({})",
            amp,
            level,
            prev
        );
        prev = level;
    }
}

#[test]
fn output_is_bounded_zero_to_one() {
    for amplitude in [-100.0_f32, -1.0, 0.0, 0.5, 1.0, 100.0] {
        let samples: Vec<f32> = vec![amplitude; 512];
        let level = rms_normalized(&samples);
        assert!(
            (0.0..=1.0).contains(&level),
            "level {} for amplitude {} out of bounds",
            level,
            amplitude
        );
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo test rms 2>&1 | tail -10
cargo test --test audio_rms 2>&1 | tail -5
```

Expected: 4 unit tests + 2 integration tests pass (`silence_yields_zero`, `full_scale_sine_yields_one`, `empty_returns_zero`, `quiet_noise_maps_to_low_range`, `monotonic_increase_with_amplitude`, `output_is_bounded_zero_to_one`).

If `quiet_noise_maps_to_low_range` fails (level might land at exactly 0 or above 0.2 depending on the exact synthetic noise), adjust the assertion bound — but examine the actual value first; it's a hint about whether the formula matches the macOS one.

- [ ] **Step 5: Commit**

```bash
git add linux/src/audio.rs linux/src/lib.rs linux/tests/audio_rms.rs
git commit -m "feat(linux): add RMS normalization matching macOS dB mapping"
```

---

## Task 1.3: cpal audio capture

**Files:**
- Modify: `linux/src/audio.rs`

This task adds the `Capture` struct that opens a cpal input stream, fans raw `f32` samples out via a `crossbeam_channel::Sender<Vec<f32>>`, and computes per-buffer RMS. The cpal callback runs on a high-priority thread — keep work minimal.

- [ ] **Step 1: Add the `Capture` struct to `linux/src/audio.rs`**

Append below the existing `rms_normalized` function (before the `#[cfg(test)]` block):

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Sender;

use crate::error::{AppError, AppResult};

/// Audio buffer + RMS level published by the capture callback.
pub struct AudioChunk {
    /// Samples in the device's native format (interleaved if multi-channel),
    /// converted to f32 in the range [-1, 1].
    pub samples: Vec<f32>,
    /// Native sample rate as reported by cpal.
    pub sample_rate: u32,
    /// Number of channels (1 or 2 typically).
    pub channels: u16,
    /// RMS level of this buffer, normalized to [0, 1] per `rms_normalized`.
    pub level: f32,
}

/// Live audio capture wrapper. Drop to stop the stream.
pub struct Capture {
    _stream: Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Capture {
    /// Open the default input device and start streaming.
    /// Each buffer is sent through `tx` along with the RMS level.
    /// The cpal callback returns immediately after sending; if the channel
    /// is full (downstream backed up), the buffer is silently dropped —
    /// this is the right behavior for real-time audio: never block the
    /// audio thread.
    pub fn start(tx: Sender<AudioChunk>) -> AppResult<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AppError::NoMicrophone("no default input device".into()))?;

        let supported = device
            .default_input_config()
            .map_err(|e| AppError::NoMicrophone(format!("query default config: {}", e)))?;

        let config: StreamConfig = supported.clone().into();
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        tracing::info!(
            device = %device.name().unwrap_or_else(|_| "<unknown>".into()),
            sample_rate,
            channels,
            format = ?supported.sample_format(),
            "opening input stream"
        );

        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, tx, sample_rate, channels)?,
            SampleFormat::I16 => build_stream::<i16>(&device, &config, tx, sample_rate, channels)?,
            SampleFormat::U16 => build_stream::<u16>(&device, &config, tx, sample_rate, channels)?,
            other => {
                return Err(AppError::NoMicrophone(format!(
                    "unsupported sample format: {:?}",
                    other
                )));
            }
        };

        stream
            .play()
            .map_err(|e| AppError::NoMicrophone(format!("starting stream: {}", e)))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
            channels,
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    tx: Sender<AudioChunk>,
    sample_rate: u32,
    channels: u16,
) -> AppResult<Stream>
where
    T: Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let err_fn = |e| tracing::warn!(error = ?e, "cpal stream error");
    let data_fn = move |data: &[T], _: &cpal::InputCallbackInfo| {
        let samples: Vec<f32> = data
            .iter()
            .map(|&s| <f32 as cpal::FromSample<T>>::from_sample(s))
            .collect();
        let level = rms_normalized(&samples);
        let chunk = AudioChunk {
            samples,
            sample_rate,
            channels,
            level,
        };
        // try_send: drop if downstream is backed up; never block audio thread
        let _ = tx.try_send(chunk);
    };

    device
        .build_input_stream(config, data_fn, err_fn, None)
        .map_err(|e| AppError::NoMicrophone(format!("build_input_stream: {}", e)))
}
```

- [ ] **Step 2: Verify the build still compiles**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo build 2>&1 | tail -5
```

Expected: clean build. Cpal pulls in ALSA dev headers — if missing, the user may need `sudo apt install libasound2-dev libjack-jackd2-dev`. The implementer subagent should report this error with the exact compiler output so the controller can advise.

If you see `Pre-compiled binary file not found: alsa.h`, STOP and report `BLOCKED` — system deps need installation.

- [ ] **Step 3: Manual smoke test (capture → log)**

Write a temporary throwaway in `linux/examples/audio_smoke.rs`:

```rust
use crossbeam_channel::bounded;
use voice_input::audio::Capture;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let (tx, rx) = bounded(32);
    let _capture = Capture::start(tx)?;
    println!("listening for 3 seconds — speak or make noise...");
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(3) {
        if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
            print!("level={:.2} samples={} ", chunk.level, chunk.samples.len());
        }
    }
    println!();
    Ok(())
}
```

Run it:

```bash
RUST_LOG=info cargo run --example audio_smoke 2>&1 | head -20
```

Expected: log line `opening input stream device=... sample_rate=... channels=...`, then 3 seconds of `level=X.XX samples=NNNN` printouts that visibly change when you speak. If `level` stays at exactly 0.0 throughout, the microphone is muted or the audio path is broken — investigate before proceeding.

- [ ] **Step 4: Delete the example file (it was a one-shot smoke test)**

```bash
rm linux/examples/audio_smoke.rs
rmdir linux/examples 2>/dev/null || true
```

- [ ] **Step 5: Commit**

```bash
git add linux/src/audio.rs
git commit -m "feat(linux): add cpal audio capture with RMS-tagged chunks"
```

---

## Task 1.4: Resample to 16 kHz mono f32

**Files:**
- Modify: `linux/src/audio.rs`

Whisper expects 16 kHz mono. Most mics give 44.1 or 48 kHz, often stereo. We resample with rubato and downmix channels by averaging.

- [ ] **Step 1: Add resampler to `linux/src/audio.rs`** (append after the `Capture` impl, before the `#[cfg(test)]` block)

```rust
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Downmix to mono by averaging interleaved channels, then resample to
/// `WHISPER_SAMPLE_RATE` (16 kHz) using a high-quality sinc interpolator.
///
/// Returns the resampled mono f32 samples ready for VAD / whisper.
pub struct Resampler16kMono {
    resampler: SincFixedIn<f32>,
    src_channels: usize,
    /// Carries leftover unprocessed mono samples between calls.
    mono_buf: Vec<f32>,
    chunk_size: usize,
}

impl Resampler16kMono {
    pub fn new(input_rate: u32, channels: u16) -> AppResult<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let chunk_size = 1024_usize;
        let resampler = SincFixedIn::<f32>::new(
            WHISPER_SAMPLE_RATE as f64 / input_rate as f64,
            2.0,
            params,
            chunk_size,
            1, // mono output
        )
        .map_err(|e| AppError::Config(format!("resampler init: {}", e)))?;

        Ok(Self {
            resampler,
            src_channels: channels as usize,
            mono_buf: Vec::with_capacity(chunk_size * 4),
            chunk_size,
        })
    }

    /// Process an interleaved multi-channel buffer at the input rate.
    /// Returns 16 kHz mono samples; may return fewer than expected if
    /// not enough samples have accumulated for a full resampler chunk.
    pub fn process(&mut self, interleaved: &[f32]) -> AppResult<Vec<f32>> {
        // Downmix to mono by averaging channels
        let frames = interleaved.len() / self.src_channels;
        self.mono_buf.reserve(frames);
        for frame in interleaved.chunks_exact(self.src_channels) {
            let avg = frame.iter().sum::<f32>() / self.src_channels as f32;
            self.mono_buf.push(avg);
        }

        let mut output = Vec::new();
        while self.mono_buf.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.mono_buf.drain(..self.chunk_size).collect();
            let resampled = self
                .resampler
                .process(&[chunk], None)
                .map_err(|e| AppError::Config(format!("resample: {}", e)))?;
            output.extend_from_slice(&resampled[0]);
        }
        Ok(output)
    }
}
```

- [ ] **Step 2: Add tests for the resampler at the end of `#[cfg(test)]` block in `linux/src/audio.rs`**

```rust
    #[test]
    fn resampler_48k_stereo_to_16k_mono_passes_through() {
        let mut r = Resampler16kMono::new(48_000, 2).unwrap();
        // 4800 stereo frames = 100ms at 48kHz
        let interleaved: Vec<f32> = (0..4800)
            .flat_map(|i| {
                let s = (i as f32 * 0.01).sin();
                vec![s, s] // identical left & right
            })
            .collect();
        let out = r.process(&interleaved).unwrap();
        // Roughly 100ms at 16kHz = ~1600 samples — allow some slack for the resampler's internal buffering
        assert!(
            out.len() > 1000 && out.len() < 2000,
            "expected ~1600 output samples, got {}",
            out.len()
        );
        // Output is mono — values should be similar magnitude to input (sin range)
        let max = out.iter().fold(0.0_f32, |a, &b| a.max(b.abs()));
        assert!(max > 0.5 && max < 1.5, "max amplitude unexpected: {}", max);
    }

    #[test]
    fn resampler_44_1k_mono_at_correct_target_rate() {
        let mut r = Resampler16kMono::new(44_100, 1).unwrap();
        let mono: Vec<f32> = (0..22_050).map(|i| (i as f32 * 0.02).sin()).collect(); // ~500ms
        let out = r.process(&mono).unwrap();
        // ~500ms at 16kHz = ~8000 samples; allow ±15% for buffering
        assert!(
            out.len() > 6_500 && out.len() < 9_500,
            "expected ~8000 samples, got {}",
            out.len()
        );
    }
```

- [ ] **Step 3: Run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo test --lib audio 2>&1 | tail -10
```

Expected: all previous audio tests still pass + 2 new resampler tests pass.

If rubato's API has shifted (e.g., `SincInterpolationParameters` no longer exists or has different field names), STOP and report `NEEDS_CONTEXT` with the compiler error — do NOT silently change parameter names.

- [ ] **Step 4: Commit**

```bash
git add linux/src/audio.rs
git commit -m "feat(linux): resample audio to 16kHz mono for whisper input"
```

---

## Task 1.5: Silero VAD slicer

**Files:**
- Create: `linux/src/speech/mod.rs`
- Create: `linux/src/speech/vad.rs`
- Modify: `linux/src/lib.rs` (add `pub mod speech;`)
- Create: `linux/tests/vad_slicing.rs`

The VAD takes a continuous 16 kHz mono f32 stream and emits `Vec<f32>` slices each time it detects ≥300 ms of trailing silence. Max slice length is 30 s (whisper's window).

- [ ] **Step 1: Update `linux/src/lib.rs`**

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod speech;
pub mod tray;
```

- [ ] **Step 2: Create `linux/src/speech/mod.rs`**

```rust
//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.

pub mod vad;
```

- [ ] **Step 3: Create `linux/src/speech/vad.rs`**

```rust
use voice_activity_detector::VoiceActivityDetector;

use crate::error::{AppError, AppResult};

/// Sample rate of the input audio for VAD. Whisper expects 16 kHz, so we
/// always feed at this rate.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// Window size (number of samples) per VAD inference. Silero is trained
/// at 16 kHz with 512-sample windows (= 32 ms).
const VAD_WINDOW: usize = 512;

/// How many trailing-silence windows close a segment (≥300 ms).
/// 300 ms / 32 ms ≈ 10 windows.
const SILENCE_WINDOWS_TO_CLOSE: usize = 10;

/// Max segment length in samples (30 s at 16 kHz = whisper's context window).
const MAX_SEGMENT_SAMPLES: usize = 30 * VAD_SAMPLE_RATE as usize;

/// Speech probability threshold. Silero outputs 0.0–1.0; >0.5 = speech.
const SPEECH_THRESHOLD: f32 = 0.5;

/// Streaming VAD slicer. Feed samples via `push`; complete segments are
/// returned by `drain` (call after every push to retrieve any closed slice).
pub struct VadSlicer {
    vad: VoiceActivityDetector,
    /// Samples buffered for the next VAD window inference.
    window_buf: Vec<f32>,
    /// Samples accumulated in the current speech segment.
    segment: Vec<f32>,
    /// Number of consecutive silence windows seen.
    silence_count: usize,
    /// True when we've seen at least one speech window in the current segment.
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

    /// Push samples and return any completed segments.
    pub fn push(&mut self, samples: &[f32]) -> AppResult<Vec<Vec<f32>>> {
        let mut completed = Vec::new();
        for &s in samples {
            self.window_buf.push(s);
            if self.window_buf.len() >= VAD_WINDOW {
                let prob = self.vad.predict(self.window_buf.iter().copied());
                let is_speech = prob >= SPEECH_THRESHOLD;
                self.segment.extend_from_slice(&self.window_buf);
                self.window_buf.clear();

                if is_speech {
                    self.in_segment = true;
                    self.silence_count = 0;
                } else if self.in_segment {
                    self.silence_count += 1;
                }

                let should_close = (self.in_segment
                    && self.silence_count >= SILENCE_WINDOWS_TO_CLOSE)
                    || self.segment.len() >= MAX_SEGMENT_SAMPLES;
                if should_close {
                    completed.push(std::mem::take(&mut self.segment));
                    self.segment.reserve(MAX_SEGMENT_SAMPLES);
                    self.silence_count = 0;
                    self.in_segment = false;
                }
            }
        }
        Ok(completed)
    }

    /// Force-emit any pending segment (e.g., on shutdown).
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        if self.in_segment && !self.segment.is_empty() {
            self.in_segment = false;
            self.silence_count = 0;
            Some(std::mem::take(&mut self.segment))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate samples for `duration_ms` of either silence or a 1 kHz sine.
    fn samples_for(duration_ms: usize, is_speech: bool) -> Vec<f32> {
        let n = duration_ms * VAD_SAMPLE_RATE as usize / 1000;
        if is_speech {
            // Use a strong amplitude sine; Silero may or may not call it "speech"
            // but the test below is structural — it works regardless of model verdict.
            (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / VAD_SAMPLE_RATE as f32).sin() * 0.5)
                .collect()
        } else {
            vec![0.0; n]
        }
    }

    #[test]
    fn instantiates_without_error() {
        let _v = VadSlicer::new().expect("init");
    }

    #[test]
    fn pure_silence_yields_no_segments() {
        let mut v = VadSlicer::new().unwrap();
        let silence = samples_for(2000, false);
        let segments = v.push(&silence).unwrap();
        assert!(segments.is_empty(), "silence produced segments");
    }

    #[test]
    fn flush_returns_none_when_idle() {
        let mut v = VadSlicer::new().unwrap();
        assert!(v.flush().is_none());
    }
}
```

- [ ] **Step 4: Write the integration test in `linux/tests/vad_slicing.rs`**

```rust
use voice_input::speech::vad::{VAD_SAMPLE_RATE, VadSlicer};

/// Generate a real-ish speech waveform: superimposed harmonics with envelope.
/// Silero is trained on actual speech; a pure sine won't reliably trigger
/// the speech class. This generates something closer to a vowel.
fn fake_speech(duration_ms: usize) -> Vec<f32> {
    let n = duration_ms * VAD_SAMPLE_RATE as usize / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / VAD_SAMPLE_RATE as f32;
            // Formant-ish stack: 200 Hz fundamental + harmonics
            let v = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.3
                + (2.0 * std::f32::consts::PI * 800.0 * t).sin() * 0.2
                + (2.0 * std::f32::consts::PI * 2400.0 * t).sin() * 0.1;
            // Slight tremolo for natural feel
            v * (1.0 + 0.1 * (2.0 * std::f32::consts::PI * 5.0 * t).sin())
        })
        .collect()
}

fn silence(duration_ms: usize) -> Vec<f32> {
    vec![0.0; duration_ms * VAD_SAMPLE_RATE as usize / 1000]
}

#[test]
fn long_silence_produces_no_segments() {
    let mut v = VadSlicer::new().unwrap();
    let segments = v.push(&silence(3000)).unwrap();
    assert!(
        segments.is_empty(),
        "silence yielded {} segment(s)",
        segments.len()
    );
}

#[test]
fn flush_returns_none_after_silence_only() {
    let mut v = VadSlicer::new().unwrap();
    let _ = v.push(&silence(2000)).unwrap();
    assert!(v.flush().is_none());
}

#[test]
fn fake_speech_then_silence_produces_at_least_one_segment_or_flush_yields_one() {
    // Silero may or may not classify fake_speech as speech. This test
    // accepts BOTH outcomes:
    //   - if Silero detects speech: a segment closes after trailing silence
    //   - if Silero does not: flush returns None, and that's still consistent
    //
    // The point of this test is to verify the slicer doesn't crash on
    // realistic-shaped input. Real speech detection is verified in the
    // end-to-end smoke test (Task 1.9).
    let mut v = VadSlicer::new().unwrap();
    let speech = fake_speech(1500);
    let trailing = silence(500);
    let segs1 = v.push(&speech).unwrap();
    let segs2 = v.push(&trailing).unwrap();
    let flushed = v.flush();
    let total = segs1.len() + segs2.len() + flushed.is_some() as usize;
    // 0 is valid (Silero rejected fake speech), >= 1 is valid (it detected).
    // This test just verifies no panic and no nonsensical output (e.g. 50 segments).
    assert!(total <= 3, "unexpected segment count: {}", total);
}
```

- [ ] **Step 5: Run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo test --test vad_slicing 2>&1 | tail -10
cargo test --lib speech 2>&1 | tail -10
```

Expected: 3 lib tests + 3 integration tests pass. **First run downloads the bundled Silero ONNX model** (small, <2 MB) — may take a few seconds.

If `voice_activity_detector` API has shifted (different builder method names, different chunk size constraint), STOP and report `NEEDS_CONTEXT`. Do not improvise.

- [ ] **Step 6: Commit**

```bash
git add linux/src/speech linux/src/lib.rs linux/tests/vad_slicing.rs
git commit -m "feat(linux): add Silero VAD slicer with 300ms trailing-silence segmenting"
```

---

## Task 1.6: Whisper worker thread

**Files:**
- Create: `linux/src/speech/worker.rs`
- Modify: `linux/src/speech/mod.rs` (add `pub mod worker;`)
- Modify: `linux/src/config.rs` (add `resolve_model_path` helper)

The worker owns a `WhisperContext` on a dedicated thread (state is `!Send`). It receives audio slices through a `crossbeam_channel::Receiver<Vec<f32>>`, transcribes each, and sends `String` results out through another channel.

- [ ] **Step 1: Add `resolve_model_path` to `linux/src/config.rs`**

Append inside `impl Config { ... }` (before the closing brace, after `save_to`):

```rust
    /// Resolve the whisper model file path:
    /// 1. `$VOICE_INPUT_MODEL_PATH` env var if set
    /// 2. `whisper_model_path` field if `Some`
    /// 3. `~/.local/share/voice-input/models/ggml-{whisper_model_size}.bin`
    pub fn resolve_model_path(&self) -> AppResult<PathBuf> {
        if let Ok(env) = std::env::var("VOICE_INPUT_MODEL_PATH") {
            if !env.is_empty() {
                return Ok(PathBuf::from(env));
            }
        }
        if let Some(ref p) = self.whisper_model_path {
            return Ok(p.clone());
        }
        let dirs = directories::ProjectDirs::from("com", "yetone", "voice-input")
            .ok_or_else(|| AppError::Config("cannot resolve XDG data dir".into()))?;
        Ok(dirs
            .data_dir()
            .join("models")
            .join(format!("ggml-{}.bin", self.whisper_model_size)))
    }
```

- [ ] **Step 2: Update `linux/src/speech/mod.rs`**

```rust
//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.

pub mod vad;
pub mod worker;
```

- [ ] **Step 3: Create `linux/src/speech/worker.rs`**

```rust
use std::path::Path;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::{AppError, AppResult};

/// Spawn a worker thread that owns a WhisperContext and transcribes audio
/// slices arriving on `slices_rx`, emitting `String` segments on `text_tx`.
/// Returns a `JoinHandle` so the caller can join on shutdown.
///
/// On error during initialization, returns immediately. Errors during
/// per-slice inference are logged and skipped — the worker keeps running.
pub fn spawn(
    model_path: &Path,
    language_hint: String,
    slices_rx: Receiver<Vec<f32>>,
    text_tx: Sender<String>,
) -> AppResult<JoinHandle<()>> {
    if !model_path.exists() {
        return Err(AppError::ModelMissing {
            path: model_path.to_path_buf(),
        });
    }
    let model_path_string = model_path.to_string_lossy().into_owned();

    let ctx = WhisperContext::new_with_params(&model_path_string, WhisperContextParameters::default())
        .map_err(|e| AppError::WhisperFailed(format!("load model {}: {}", model_path_string, e)))?;

    let handle = thread::Builder::new()
        .name("whisper-worker".into())
        .spawn(move || run(ctx, language_hint, slices_rx, text_tx))
        .map_err(|e| AppError::WhisperFailed(format!("spawn worker thread: {}", e)))?;

    Ok(handle)
}

fn run(
    ctx: WhisperContext,
    language_hint: String,
    slices_rx: Receiver<Vec<f32>>,
    text_tx: Sender<String>,
) {
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "whisper create_state failed; worker exiting");
            return;
        }
    };

    while let Ok(slice) = slices_rx.recv() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if !language_hint.is_empty() {
            params.set_language(Some(&language_hint));
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
            // Downstream dropped — caller went away, exit cleanly.
            tracing::info!("whisper worker: text channel closed, exiting");
            return;
        }
    }
    tracing::info!("whisper worker: slice channel closed, exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_model_path_returns_model_missing_error() {
        let (_, slices_rx) = crossbeam_channel::bounded(1);
        let (text_tx, _) = crossbeam_channel::bounded(1);
        let path = PathBuf::from("/nonexistent/ggml-tiny.bin");
        let err = spawn(&path, "zh".into(), slices_rx, text_tx).unwrap_err();
        match err {
            AppError::ModelMissing { path: p } => assert!(p.to_string_lossy().contains("nonexistent")),
            other => panic!("expected ModelMissing, got {:?}", other),
        }
    }

    /// Real inference test — requires a downloaded whisper model.
    /// Run with: cargo test --lib -- --ignored
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
        let (slices_tx, slices_rx) = crossbeam_channel::bounded(1);
        let (text_tx, text_rx) = crossbeam_channel::bounded(1);
        let handle = spawn(&path, "en".into(), slices_rx, text_tx).unwrap();

        // 3 seconds of silence at 16kHz
        let silence = vec![0.0_f32; 16_000 * 3];
        slices_tx.send(silence).unwrap();

        // Expect either no output or very short output; either way, no panic.
        let _ = text_rx.recv_timeout(std::time::Duration::from_secs(30));
        drop(slices_tx);
        handle.join().unwrap();
    }

    fn dirs_for_test() -> PathBuf {
        directories::ProjectDirs::from("com", "yetone", "voice-input")
            .map(|d| d.data_dir().join("models"))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}
```

- [ ] **Step 4: Run the build (whisper-rs will compile if not already cached)**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo build 2>&1 | tail -5
cargo test --lib speech::worker::tests::missing_model_path_returns_model_missing_error 2>&1 | tail -5
```

Expected: clean build (whisper.cpp may take 30-60s on first compile). The single non-ignored test passes.

If whisper-rs API has shifted (e.g., `FullParams::new` signature change, or `WhisperContext::new_with_params` renamed), STOP and report `NEEDS_CONTEXT`. Do not silently rename methods.

- [ ] **Step 5: Commit**

```bash
git add linux/src/speech linux/src/config.rs
git commit -m "feat(linux): add whisper worker thread with channel-based inference"
```

---

## Task 1.7: Wire the pipeline into `run_transcribe`

**Files:**
- Modify: `linux/src/main.rs` (implement the `run_transcribe` function body)
- Modify: `linux/src/speech/mod.rs` (add `start_pipeline` helper that wires everything)

- [ ] **Step 1: Add `start_pipeline` to `linux/src/speech/mod.rs`**

Replace the contents with:

```rust
//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.

pub mod vad;
pub mod worker;

use std::path::Path;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Receiver};

use crate::audio::{AudioChunk, Capture, Resampler16kMono};
use crate::error::{AppError, AppResult};

/// Handle returned by `start_pipeline`. Drop to begin teardown; call
/// `join` to wait for clean shutdown of all worker threads.
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    _capture: Capture,
    vad_handle: Option<JoinHandle<()>>,
    whisper_handle: Option<JoinHandle<()>>,
}

impl PipelineHandle {
    /// Wait for the VAD and whisper workers to finish. Call after dropping
    /// or otherwise closing the slice/text channels.
    pub fn join(mut self) {
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.whisper_handle.take() {
            let _ = h.join();
        }
    }
}

/// Start the audio → resample → VAD → whisper pipeline.
/// Returns a handle including the text receiver.
pub fn start_pipeline(model_path: &Path, language_hint: String) -> AppResult<PipelineHandle> {
    let (audio_tx, audio_rx) = bounded::<AudioChunk>(64);
    let (slice_tx, slice_rx) = bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = bounded::<String>(8);

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
        _capture: capture,
        vad_handle: Some(vad_handle),
        whisper_handle: Some(whisper_handle),
    })
}

fn run_vad_resample(
    audio_rx: Receiver<AudioChunk>,
    slice_tx: crossbeam_channel::Sender<Vec<f32>>,
    input_rate: u32,
    input_channels: u16,
) {
    let mut resampler = match Resampler16kMono::new(input_rate, input_channels) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "resampler init failed");
            return;
        }
    };
    let mut slicer = match vad::VadSlicer::new() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "vad init failed");
            return;
        }
    };

    while let Ok(chunk) = audio_rx.recv() {
        let mono16k = match resampler.process(&chunk.samples) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "resample failed");
                continue;
            }
        };
        let segments = match slicer.push(&mono16k) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "vad push failed");
                continue;
            }
        };
        for seg in segments {
            if slice_tx.send(seg).is_err() {
                tracing::info!("vad: downstream closed, exiting");
                return;
            }
        }
    }

    if let Some(final_segment) = slicer.flush() {
        let _ = slice_tx.send(final_segment);
    }
    tracing::info!("vad: audio channel closed, exiting");
}
```

- [ ] **Step 2: Replace `run_transcribe` in `linux/src/main.rs`**

Replace the existing placeholder `fn run_transcribe(_cfg: Config) -> anyhow::Result<()> { ... }` with:

```rust
fn run_transcribe(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg.resolve_model_path().context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting transcribe pipeline");

    let pipeline = voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone())
        .context("starting speech pipeline")?;

    tracing::info!("listening — speak into the default mic; press Ctrl+C to stop");

    let mut segment_count = 0_usize;
    let (interrupt_tx, interrupt_rx) = crossbeam_channel::bounded::<()>(1);

    ctrlc::set_handler(move || {
        let _ = interrupt_tx.try_send(());
    })
    .context("installing Ctrl+C handler")?;

    loop {
        crossbeam_channel::select! {
            recv(pipeline.text_rx) -> msg => {
                match msg {
                    Ok(text) => {
                        segment_count += 1;
                        println!("[segment {}] {}", segment_count, text);
                    }
                    Err(_) => {
                        tracing::info!("pipeline closed");
                        break;
                    }
                }
            }
            recv(interrupt_rx) -> _ => {
                tracing::info!("SIGINT received; shutting down pipeline");
                break;
            }
        }
    }

    pipeline.join();
    tracing::info!("pipeline shutdown complete; transcribed {} segments", segment_count);
    Ok(())
}
```

- [ ] **Step 3: Add `ctrlc` dependency**

In `linux/Cargo.toml`, add to `[dependencies]`:

```toml
ctrlc = "3"
```

Sort the section alphabetically.

(`tokio::signal::ctrl_c` only works inside a tokio runtime; the transcribe path is fully synchronous so we use the standalone `ctrlc` crate.)

- [ ] **Step 4: Build and run a brief smoke test (without a real model — expect ModelMissing error)**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo build 2>&1 | tail -5
# Smoke test the error path: no model file should produce a clear error
rm -rf ~/.local/share/voice-input/models  # ensure no model is present
RUST_LOG=info cargo run -- transcribe 2>&1 | head -10
```

Expected: log line `starting transcribe pipeline model=...`, then error:
```
Error: starting speech pipeline

Caused by:
    whisper model file missing at /home/.../ggml-small.bin
```

(Exact wording may vary; the key signal is `ModelMissing` error with the resolved path.)

- [ ] **Step 5: Commit**

```bash
git add linux/Cargo.toml linux/Cargo.lock linux/src/speech/mod.rs linux/src/main.rs
git commit -m "feat(linux): wire audio→VAD→whisper pipeline behind transcribe subcommand"
```

---

## Task 1.8: README — download model + run instructions

**Files:**
- Modify: `linux/README.md`

- [ ] **Step 1: Update `linux/README.md`** — replace the entire file content with:

````markdown
# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **Phase 1** — CLI transcription pipeline. Tray still works (default invocation). No hotkey or paste injection yet (Phase 2). See `../implementation/` for the phased build plan.

## Build

Requires Rust 1.83+, `cmake`, `libclang`, and `cc`/`gcc`. On first build, whisper.cpp compiles from source (≈30–60 s).

```bash
cd linux
cargo build --release
```

System packages (Debian/Ubuntu): `sudo apt install cmake clang libclang-dev libasound2-dev`.

## Download a whisper model

Phase 1 expects a `ggml-*.bin` whisper model on disk. Default path: `~/.local/share/voice-input/models/ggml-small.bin`. To download:

```bash
mkdir -p ~/.local/share/voice-input/models
curl -L -o ~/.local/share/voice-input/models/ggml-small.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

Other sizes: `tiny` (75 MB), `base` (142 MB), `small` (466 MB, default), `medium` (1.5 GB). Match the size to the `whisper_model_size` value in `~/.config/voice-input/config.toml`.

Override the path with `VOICE_INPUT_MODEL_PATH=/some/where/model.bin` or by setting `whisper_model_path` in the config file.

## Run

### Tray mode (Phase 0 behavior)

```bash
RUST_LOG=info cargo run
```

A tray icon appears in your system tray (KDE Plasma) or waybar (sway / hyprland — needs the `tray` module).

### Transcribe mode (Phase 1)

```bash
RUST_LOG=info cargo run -- transcribe
```

Reads from the default microphone, slices speech on natural pauses (≥300 ms silence), and prints each transcribed segment. Press Ctrl+C to stop.

Example output:
```
[segment 1] 你好世界
[segment 2] this is a test
```

## Compositor support

- **KDE Plasma 6**: target compositor, built-in StatusNotifierItem host.
- **sway**: requires waybar with `tray` module.
- **hyprland**: requires waybar / ironbar / Riftbar with `tray` module.
- **GNOME**: **not supported.** Mutter lacks `wlr-layer-shell` (needed in Phase 3).

## Config

`~/.config/voice-input/config.toml` — created on first run. Edit and restart to change. Notable keys:

- `language_hint = "zh"` — passed to whisper as a hint (`"en"`, `"ja"`, etc., or empty for auto-detect)
- `whisper_model_size = "small"` — determines the default model path

## Project layout

See `../plans/voice-input-linux.md` for the full design and `../implementation/` for per-phase implementation plans.
````

- [ ] **Step 2: Commit**

```bash
git add linux/README.md
git commit -m "docs(linux): document model download and transcribe CLI"
```

---

## Task 1.9: Manual end-to-end smoke test (user-driven)

This is the Phase 1 acceptance gate. The controller will hand this to the user.

- [ ] **Step 1: Verify model is downloaded**

```bash
ls -la ~/.local/share/voice-input/models/
```

Expected: a `ggml-small.bin` (or matching size). If absent, run the curl command from the README first.

- [ ] **Step 2: Run transcribe and speak**

```bash
cd /home/desmond/Repos/voice-input-src/linux
RUST_LOG=info cargo run --release -- transcribe
```

(Use `--release` — debug whisper is 5–10× slower.)

Speak short Chinese, English, or mixed phrases with deliberate ≥1 s pauses between them. Expected output:

```
INFO voice_input: config loaded ...
INFO voice_input::main: starting transcribe pipeline model=...
INFO voice_input::audio: opening input stream device=... sample_rate=44100 channels=2 format=F32
INFO voice_input::main: listening — speak into the default mic; press Ctrl+C to stop

[segment 1] (your first phrase)
[segment 2] (your second phrase)
...
```

Press Ctrl+C to stop. Expected:
```
INFO voice_input::main: SIGINT received; shutting down pipeline
INFO voice_input::main: pipeline shutdown complete; transcribed N segments
```

- [ ] **Step 3: Verify the tray path still works (Phase 0 regression check)**

```bash
RUST_LOG=info cargo run
```

Should show the tray icon and `voice-input running` log line. Quit via the tray.

- [ ] **Step 4: Report findings**

Phase 1 passes if:
- ≥1 spoken segment was transcribed correctly (whisper inaccuracy aside, the pipeline produced text)
- No panics, no stuck pipeline
- Ctrl+C cleanly shuts down
- Tray mode still works (no regression)

If any of these fail, gather the log and report — the implementer subagent can be dispatched to fix specifics.

---

## Task 1.10: Final verification + push

- [ ] **Step 1: Run all tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo test 2>&1 | grep "test result"
```

Expected (Phase 0 + Phase 1):
- `test result: ok. 8 passed` (Phase 0 lib tests, expanded with new audio + speech tests — actual count will be higher; record it)
- `test result: ok. N passed` for integration tests (config_roundtrip, audio_rms, vad_slicing — 7 integration tests total)

Goal: ≥18 tests passing total, 0 failing, 1 ignored (whisper inference test).

- [ ] **Step 2: Release build, no warnings from our crate**

```bash
cargo build --release 2>&1 | grep -E "warning|error" | grep -v "Compiling" | head -20
```

Expected: no warnings or errors from `voice-input` crate (dependency warnings allowed).

- [ ] **Step 3: Format and clippy**

```bash
cargo fmt --check 2>&1 || (cargo fmt && cd /home/desmond/Repos/voice-input-src && git add -u linux/ && git commit -m "style(linux): cargo fmt")
cd /home/desmond/Repos/voice-input-src/linux
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

If clippy reports issues:
- Trivial fixes (needless borrow, redundant clone, missing const): apply, ensure tests still pass, commit as `chore(linux): fix clippy findings`.
- Non-trivial findings: STOP and report DONE_WITH_CONCERNS — don't restructure code.

- [ ] **Step 4: Push the branch (Phase 1 branch is `linux/phase-1-audio-vad-speech`)**

Before push, confirm the branch:

```bash
cd /home/desmond/Repos/voice-input-src
git branch --show-current
```

If currently on `main`, the controller forgot to create a feature branch — STOP and request a branch be created. Otherwise:

```bash
git push -u origin linux/phase-1-audio-vad-speech
```

- [ ] **Step 5: Final verification**

```bash
git log origin/main..HEAD --oneline
git status
```

Phase 1 complete when:
- All commits present on the feature branch
- Working tree clean
- Branch tracks `origin/linux/phase-1-audio-vad-speech`
- Manual smoke test (Task 1.9) passed

---

## Self-Review Notes

**Spec coverage check** (from `plans/voice-input-linux.md` Phase 1 + module port map):
- ✅ cpal capture + RMS → Tasks 1.2, 1.3
- ✅ Resample to 16 kHz mono → Task 1.4
- ✅ Silero VAD slicer → Task 1.5
- ✅ whisper-rs worker thread → Task 1.6
- ✅ "CLI: `cargo run -- transcribe-stdin`" — implemented as `cargo run -- transcribe` (no stdin; reads mic). The "-stdin" suffix in the design doc was imprecise — the actual data source is the mic. Documented in Task 1.1 + README.
- ✅ Print segments to stdout → Task 1.7
- ✅ "No UI" → Tasks 1.1–1.7 deliberately avoid GTK
- ✅ Language hint via whisper-rs `set_language` → Task 1.6
- ✅ Build instructions in README → Task 1.8

**Phase 0 entry conditions addressed:**
1. ✅ Threading model documented in `speech/mod.rs` and the plan header (Phase 1 uses `std::thread` + crossbeam; Phase 3 reworks `main.rs` only)
2. ✅ `lib.rs` continues flat `pub mod X;` convention
3. ✅ libclang + cmake confirmed present (entry condition #3)
4. ✅ Corrupted config decision: crash with `AppError::Config` (documented in design decisions section)

**Placeholder scan:** no "TBD", "TODO", "fill in details", or "similar to" references.

**Type consistency check:**
- `AudioChunk` used by `Capture` and consumed in `start_pipeline` ✓
- `Resampler16kMono::new(input_rate, channels)` matches usage in `run_vad_resample` ✓
- `VadSlicer::new() / push() / flush()` signatures match usage ✓
- `speech::worker::spawn(model_path, language_hint, slices_rx, text_tx)` matches caller ✓
- `start_pipeline(&Path, String)` matches caller in `run_transcribe` ✓

**Scope check:** Phase 1 deliberately excludes the Phase 0 tray expansion, hotkey binding (Phase 2), text injection (Phase 2), overlay window (Phase 3), LLM refiner (Phase 4), Settings dialog (Phase 5), first-run wizard (Phase 6), packaging (Phase 7+). These are explicit non-goals.

**Known risk:** if any of the major dep versions (`whisper-rs 0.14`, `voice_activity_detector 0.2`, `cpal 0.15`, `rubato 0.16`) have API drift compared to what this plan assumes, individual implementer subagents are instructed to STOP and report `NEEDS_CONTEXT` rather than silently editing method signatures. The controller can then update the plan with the actual API.
