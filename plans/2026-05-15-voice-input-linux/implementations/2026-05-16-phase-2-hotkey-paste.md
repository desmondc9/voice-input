# Phase 2 — Hotkey + Paste Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `voice-input listen` subcommand that runs as a foreground daemon: binds a global hotkey via the XDG `GlobalShortcuts` portal on first run, then on each press starts the Phase 1 audio→VAD→whisper pipeline, and on release drains the final segments, joins them, writes to the clipboard via `wl-clipboard-rs`, simulates Ctrl+V via `ydotool`, and restores the original clipboard ~500 ms later. Hold-to-talk into any focused application.

**Architecture:** Async tokio runtime (built explicitly inside `run_listen`, same pattern as `run_tray`) drives ashpd portal events. The portal yields an `Activated`/`Deactivated` stream — press creates a `PipelineHandle`, release destructures the handle (preserves `text_rx`), joins the worker threads (Capture-first per the Phase 1 deadlock fix), drains any buffered text, then calls `injector::inject_text` to paste. Phase 1 pipeline modules remain untouched.

**Tech Stack:**
- `ashpd = "0.13"` (XDG Desktop Portals — `GlobalShortcuts`)
- `wl-clipboard-rs = "0.9"` (Wayland clipboard, no GUI surface needed)
- `futures-util = "0.3"` (`StreamExt` for portal event streams)
- `ydotool` + `ydotoold` (external binary — required runtime dep, install script provided)
- Existing: tokio, crossbeam-channel, tracing

**Reference spec:** `plans/voice-input-linux.md` — Phase 2 section and the module-port-map rows for `hotkey.rs` and `injector.rs`. Phase 0 final review's Phase 2 entry note about portal interactive binding. Phase 1 final review's `PipelineHandle::Drop` recommendation (item #3).

**Phase 1 entry-condition carryovers** (handled in Task 2.1 below):
- Add `Drop` impl to `PipelineHandle` (Phase 1 review item #3)
- Add 3 unit tests for `Config::resolve_model_path` (Phase 1 review item #2)
- Add `drain_and_join` method to `PipelineHandle` that returns the final buffered segments (new — needed by Phase 2 release handler)

---

## File Structure (after Phase 2)

| Path | Responsibility |
|---|---|
| `linux/Cargo.toml` | Add `ashpd`, `wl-clipboard-rs`, `futures-util` |
| `linux/scripts/install-ydotool.sh` | One-shot bootstrap: apt install + udev rule + user systemd unit |
| `linux/src/lib.rs` | Add `pub mod hotkey;`, `pub mod injector;` |
| `linux/src/cli.rs` | Add `Command::Listen` variant |
| `linux/src/hotkey.rs` | NEW — ashpd `GlobalShortcuts` session: create / bind / persist / event stream |
| `linux/src/injector.rs` | NEW — wl-clipboard save+write+restore + ydotool Ctrl+V shell-out |
| `linux/src/speech/mod.rs` | MODIFY — add `Drop` impl and `drain_and_join` method on `PipelineHandle` |
| `linux/src/main.rs` | MODIFY — add `run_listen` function (tokio runtime, hotkey loop, paste handler) |
| `linux/src/config.rs` | (no struct changes — `shortcut_handle` field already exists from Phase 0; just used here) |
| `linux/README.md` | Add `Listen mode` section + `install-ydotool.sh` instructions |
| `linux/tests/resolve_model_path.rs` | NEW — 3 unit tests for the three resolution branches |

**Files NOT touched in Phase 2:** `audio.rs`, `speech/vad.rs`, `speech/worker.rs`, `error.rs` (already has `YdotoolMissing`, `PortalRevoked` variants), `app.rs`, `tray.rs`.

---

## Threading & data flow (listen mode)

```
main thread
    │
    ▼
[clap parses CLI]
    │
    ├── (no args)    → run_tray (Phase 0)
    ├── transcribe   → run_transcribe (Phase 1)
    └── listen       → run_listen (Phase 2) ────┐
                                                │
                            tokio::Runtime::new()
                                                │
                                  ┌─────────────┴─────────────┐
                                  ▼                           ▼
                            ashpd portal              ctrlc handler
                            session                   (interrupt_rx)
                                  │
                                  ▼ activated/deactivated streams
                            tokio::select! loop
                                  │
                       ┌──────────┼──────────┐
                       ▼          ▼          ▼
                   pressed:   released:   ctrl-c:
                   start      destructure  break
                   pipeline   PipelineHandle
                              ↓
                              drain_and_join → Vec<String>
                              ↓
                              injector::inject_text(joined)
                                     ↓
                              (block_in_place — uses std::thread::sleep)
```

Phase 1 pipeline modules (audio, vad, worker) are unchanged. They continue to use `std::thread` + `crossbeam_channel` (runtime-agnostic per the Phase 0 final review). The async layer is purely for ashpd portal events and tokio::select coordination.

---

## Open design decisions resolved before tasks begin

1. **Hotkey binding strategy:** Use the XDG `GlobalShortcuts` portal exclusively in Phase 2. First run shows the portal's interactive binding dialog (user picks the chord — recommended Right Ctrl). The `session_handle` is persisted to `~/.config/voice-input/config.toml::shortcut_handle` for silent restoration on subsequent runs. Compositor-binding fallback (sway/hyprland direct binds) is deferred to Phase 5 polish if portal proves unreliable.
2. **Single-shortcut MVP:** Bind exactly one shortcut named `toggle_recording` with a descriptive label. Future phases can extend.
3. **Stream-vs-collect text:** Collect-then-paste. Buffer segments while hotkey is held; on release, join with `" "` separator and paste once. Cleaner UX than multiple paste events.
4. **Ydotool key sequence:** Use the explicit evdev keycode form `ydotool key 29:1 47:1 47:0 29:0` (Ctrl-down V-down V-up Ctrl-up). More reliable than `ctrl+v` chord syntax across ydotool versions.
5. **Clipboard restore delay:** 500 ms `std::thread::sleep` after ydotool returns. The receiving application needs time to consume the clipboard via its paste handler before we restore the original contents.
6. **`PipelineHandle::Drop` semantics:** The Drop impl performs the same Capture-first → vad-join → whisper-join sequence as `join()`. Both methods use `Option::take()` so they're idempotent. Explicit `join()` is kept for callers who want to wait synchronously; `Drop` covers implicit cleanup.
7. **CLI subcommand naming:** `listen` (not `daemon` — too generic, not `holdtalk` — too cute). Final user-facing entry point.

---

## Task 2.1: Phase 1 carryovers — Drop impl, drain_and_join, resolve_model_path tests

**Files:**
- Modify: `linux/src/speech/mod.rs` (add Drop + drain_and_join)
- Create: `linux/tests/resolve_model_path.rs`

- [ ] **Step 1: Modify `linux/src/speech/mod.rs`** — replace the existing `impl PipelineHandle` block with:

```rust
impl PipelineHandle {
    /// Perform an orderly shutdown:
    /// 1. Drop the audio capture so the cpal stream stops and the
    ///    `audio_tx` Sender is released, allowing the VAD thread's
    ///    `audio_rx.recv()` to return Err.
    /// 2. Join the VAD thread (now able to exit).
    /// 3. The VAD thread, on exit, drops `slice_tx`, which lets the
    ///    whisper worker's `slices_rx.recv()` return Err.
    /// 4. Join the whisper worker thread.
    pub fn join(mut self) {
        self.shutdown_internal();
    }

    /// Shut down (same as `join`), then drain any remaining segments from
    /// `text_rx`. Returns the buffered segments in arrival order.
    ///
    /// Phase 2 uses this after a hotkey release to capture the final
    /// segments before pasting.
    pub fn drain_and_join(mut self) -> Vec<String> {
        self.shutdown_internal();
        // After both worker threads have joined, all senders for text_tx
        // have been dropped. The remaining items in text_rx are exactly
        // the segments produced before shutdown.
        let mut out = Vec::new();
        while let Ok(seg) = self.text_rx.try_recv() {
            out.push(seg);
        }
        out
    }

    fn shutdown_internal(&mut self) {
        drop(self.capture.take());
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.whisper_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        // Idempotent because shutdown_internal uses Option::take.
        self.shutdown_internal();
    }
}
```

(The struct definition above this block is unchanged. The doc comment above `pub struct PipelineHandle` should now reflect that dropping does the same as `join`; it already says this — leave as-is.)

- [ ] **Step 2: Create `linux/tests/resolve_model_path.rs`** with:

```rust
use std::path::PathBuf;

use voice_input::config::Config;

/// Set $VOICE_INPUT_MODEL_PATH for the duration of the test.
/// SAFETY: tests in this file are NOT marked #[serial], so env-var manipulation
/// is per-test by setting a unique value. Cargo test runs in parallel by
/// default; we make each test idempotent by always setting the var explicitly
/// (no test relies on the env var being unset).
fn with_env_var<R>(value: &str, f: impl FnOnce() -> R) -> R {
    let key = "VOICE_INPUT_MODEL_PATH";
    let prev = std::env::var(key).ok();
    // SAFETY: setting env vars from tests is racy with other tests reading
    // the same var; this is acceptable for our purposes because every
    // resolve_model_path test sets it explicitly. We restore after.
    unsafe {
        std::env::set_var(key, value);
    }
    let r = f();
    unsafe {
        match prev {
            Some(p) => std::env::set_var(key, p),
            None => std::env::remove_var(key),
        }
    }
    r
}

#[test]
fn env_var_override_wins() {
    let cfg = Config::default();
    with_env_var("/tmp/voice-input-test-env.bin", || {
        let path = cfg.resolve_model_path().expect("resolve");
        assert_eq!(path, PathBuf::from("/tmp/voice-input-test-env.bin"));
    });
}

#[test]
fn config_field_wins_over_default() {
    let mut cfg = Config::default();
    cfg.whisper_model_path = Some(PathBuf::from("/tmp/voice-input-test-config.bin"));
    with_env_var("", || {
        let path = cfg.resolve_model_path().expect("resolve");
        assert_eq!(path, PathBuf::from("/tmp/voice-input-test-config.bin"));
    });
}

#[test]
fn default_uses_xdg_data_dir() {
    let cfg = Config::default();
    with_env_var("", || {
        let path = cfg.resolve_model_path().expect("resolve");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with("voice-input/models/ggml-small.bin"),
            "expected XDG path ending with voice-input/models/ggml-small.bin, got {}",
            path_str
        );
    });
}
```

(`unsafe { std::env::set_var(...) }` is required by Rust 1.83+ since `set_var` is marked unsafe due to the cross-thread race risk. Acceptable in tests.)

- [ ] **Step 3: Verify tests pass**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: at least 29 lib+integration tests pass (26 from Phase 1 + 3 new resolve_model_path tests), 1 ignored.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/speech/mod.rs linux/tests/resolve_model_path.rs
git commit -m "feat(linux): add PipelineHandle Drop + drain_and_join + resolve_model_path tests"
```

---

## Task 2.2: Add Phase 2 dependencies + `listen` CLI subcommand stub

**Files:**
- Modify: `linux/Cargo.toml`
- Modify: `linux/src/cli.rs`
- Modify: `linux/src/main.rs` (add `run_listen` placeholder + dispatch)

- [ ] **Step 1: Edit `linux/Cargo.toml`** — add `ashpd`, `wl-clipboard-rs`, `futures-util` in alphabetical order. Final `[dependencies]`:

```toml
[dependencies]
anyhow = "1"
ashpd = "0.13"
clap = { version = "4", features = ["derive"] }
cpal = "0.15"
crossbeam-channel = "0.5"
ctrlc = "3"
directories = "5"
futures-util = "0.3"
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

(`hound` was already removed at the end of Phase 1; do not re-add.)

- [ ] **Step 2: Verify deps resolve and build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
```

Expected: clean `Finished` line. First build with ashpd + wl-clipboard-rs will recompile a few seconds. If any of the three new deps fail to resolve (e.g., `ashpd 0.13` not found, or feature mismatch — ashpd may require `tokio` feature explicitly), STOP and report `NEEDS_CONTEXT` with the exact cargo error. Do NOT bump versions on your own.

If ashpd build complains about `pipewire-sys` or similar (some ashpd features pull in pipewire dev headers), report. The bare `ashpd = "0.13"` should use the default features which exclude pipewire.

- [ ] **Step 3: Edit `linux/src/cli.rs`** — replace entire content with:

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
    /// Run as a foreground daemon: hold the configured global hotkey to
    /// record, release to paste the transcribed text into the focused
    /// application. First run prompts the XDG portal to bind a shortcut.
    Listen,
}
```

- [ ] **Step 4: Edit `linux/src/main.rs`** — modify the `match cli.command` block and add a `run_listen` placeholder.

Find the existing match block:

```rust
    match cli.command {
        None => run_tray(cfg),
        Some(Command::Transcribe) => run_transcribe(cfg),
    }
```

Replace with:

```rust
    match cli.command {
        None => run_tray(cfg),
        Some(Command::Transcribe) => run_transcribe(cfg),
        Some(Command::Listen) => run_listen(cfg),
    }
```

Then add a new function at the end of `main.rs`:

```rust
fn run_listen(_cfg: Config) -> anyhow::Result<()> {
    // Implemented in Task 2.5. For now, just print a placeholder so the CLI
    // dispatch is testable end-to-end.
    println!("listen subcommand: hotkey + paste wiring not yet implemented (Task 2.5)");
    Ok(())
}
```

- [ ] **Step 5: Smoke-test CLI dispatch**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo run -- --help 2>&1 | head -20
PATH="$HOME/.cargo/bin:$PATH" cargo run -- listen 2>&1 | head -5
```

Expected:
- `--help` shows both `transcribe` and `listen` subcommands
- `listen` prints the placeholder line and exits with status 0

- [ ] **Step 6: Confirm tests still pass**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: all prior tests pass (29 + 1 ignored).

- [ ] **Step 7: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/Cargo.lock linux/src/cli.rs linux/src/main.rs
git commit -m "feat(linux): add Phase 2 dependencies and listen CLI subcommand stub"
```

---

## Task 2.3: hotkey.rs — ashpd GlobalShortcuts wrapper

**Files:**
- Create: `linux/src/hotkey.rs`
- Modify: `linux/src/lib.rs` (add `pub mod hotkey;`)

This module owns an ashpd `GlobalShortcuts` proxy and a session. Public API: `create_or_restore(cfg) -> AppResult<HotkeyHandle>` which either binds a new shortcut interactively (first run) or restores from a persisted session token, plus `events()` returning a struct with two streams (activated, deactivated).

- [ ] **Step 1: Add `pub mod hotkey;` to `linux/src/lib.rs`** — final content (alphabetical, 8 lines):

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod speech;
pub mod tray;
```

- [ ] **Step 2: Create `linux/src/hotkey.rs`** with this exact content:

```rust
use ashpd::desktop::global_shortcuts::{
    Activated, Deactivated, GlobalShortcuts, NewShortcut, ShortcutsResponse,
};
use ashpd::desktop::Session;
use futures_util::stream::{Stream, StreamExt};

use crate::error::{AppError, AppResult};

/// The single shortcut id used by Phase 2.
pub const SHORTCUT_ID: &str = "toggle_recording";

/// Wraps an ashpd `GlobalShortcuts` session and exposes activated /
/// deactivated event streams. Drop the handle to end the session.
pub struct HotkeyHandle<'a> {
    proxy: GlobalShortcuts<'a>,
    _session: Session<'a, GlobalShortcuts<'a>>,
}

impl<'a> HotkeyHandle<'a> {
    /// Create a new portal session and ensure a shortcut is bound.
    /// On first run, the portal shows a binding dialog; on subsequent runs
    /// (when the portal remembers the session by name) it should not.
    ///
    /// Phase 2 does NOT persist the session handle across process restarts —
    /// each `voice-input listen` run creates a fresh session. The portal
    /// implementation generally remembers user-bound shortcuts by app id.
    pub async fn create() -> AppResult<HotkeyHandle<'a>> {
        let proxy = GlobalShortcuts::new()
            .await
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("create proxy: {e}")))?;
        let session = proxy
            .create_session()
            .await
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("create session: {e}")))?;

        let shortcut = NewShortcut::new(SHORTCUT_ID, "Hold to dictate")
            .preferred_trigger("CTRL_R");

        let response = proxy
            .bind_shortcuts(&session, &[shortcut], None)
            .await
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("bind: {e}")))?
            .response()
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("bind response: {e}")))?;

        tracing::info!(
            shortcuts = ?response.shortcuts().iter().map(|s| s.id()).collect::<Vec<_>>(),
            "portal bound shortcuts"
        );

        Ok(HotkeyHandle {
            proxy,
            _session: session,
        })
    }

    /// Stream of "pressed" events (one per key-down).
    pub async fn activated(&self) -> AppResult<impl Stream<Item = Activated> + '_> {
        self.proxy
            .receive_activated()
            .await
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("activated stream: {e}")))
    }

    /// Stream of "released" events (one per key-up).
    pub async fn deactivated(&self) -> AppResult<impl Stream<Item = Deactivated> + '_> {
        self.proxy
            .receive_deactivated()
            .await
            .map_err(|e| AppError::PortalRevoked.with_context_msg(format!("deactivated stream: {e}")))
    }
}

// Helper extension so AppError gets `.with_context_msg("...")` without
// allocating an anyhow wrapper. Defined here (private) until error.rs
// grows a real `.context(...)` API.
trait AppErrorExt {
    fn with_context_msg(self, msg: String) -> AppError;
}

impl AppErrorExt for AppError {
    fn with_context_msg(self, msg: String) -> AppError {
        match self {
            AppError::PortalRevoked => AppError::Config(format!("portal: {msg}")),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_id_constant_is_stable() {
        // Phase 5 may add more shortcut ids; this test pins the toggle_recording
        // id so subsequent phases don't accidentally rename it (which would
        // break user-bound shortcuts on upgrade).
        assert_eq!(SHORTCUT_ID, "toggle_recording");
    }
}
```

**IMPORTANT** about API drift on ashpd 0.13:
- If `ashpd::desktop::global_shortcuts::GlobalShortcuts` doesn't exist or has a different path, the implementer must STOP and report `NEEDS_CONTEXT`. Run `find ~/.cargo/registry/src -path '*ashpd-0.13*' -name '*.rs' | xargs grep -l 'GlobalShortcuts'` to find the actual module.
- If `NewShortcut::new(id, description).preferred_trigger("CTRL_R")` builder doesn't exist as shown, the implementer should report. The Phase 2 plan was written assuming ashpd 0.13's typed-builder API.
- If `bind_shortcuts(...).await?.response()?` doesn't match the actual return type, STOP and report.

The single `#[test]` is a pin — actual interaction tests require a real D-Bus session and portal backend; defer those to the manual smoke test in Task 2.7.

- [ ] **Step 3: Build and run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib hotkey 2>&1 | tail -5
```

Expected: clean build; 1 lib test passes (`shortcut_id_constant_is_stable`).

If ashpd 0.13 API drift causes build errors, STOP and report `NEEDS_CONTEXT` with the exact compiler output. Investigate the ashpd source at `~/.cargo/registry/src/index.crates.io-*/ashpd-0.13*/src/desktop/global_shortcuts.rs` for the correct method names and signatures — propose corrections in your NEEDS_CONTEXT report, don't apply them blindly.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/hotkey.rs linux/src/lib.rs
git commit -m "feat(linux): add ashpd GlobalShortcuts session wrapper"
```

---

## Task 2.4: injector.rs — clipboard save+write+restore + ydotool paste

**Files:**
- Create: `linux/src/injector.rs`
- Modify: `linux/src/lib.rs` (add `pub mod injector;`)

This module owns clipboard handling and the ydotool shell-out. It exposes `verify_available()` (called once at startup) and `inject_text(s: &str)` (called per release).

- [ ] **Step 1: Update `linux/src/lib.rs`** — final content (9 lines, alphabetical):

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod injector;
pub mod speech;
pub mod tray;
```

- [ ] **Step 2: Create `linux/src/injector.rs`** with this exact content:

```rust
use std::io::Read;
use std::process::Command;
use std::time::Duration;

use wl_clipboard_rs::copy::{MimeType as CopyMime, Options as CopyOptions, Source};
use wl_clipboard_rs::paste::{
    get_contents, ClipboardType, MimeType as PasteMime, Seat as PasteSeat,
};

use crate::error::{AppError, AppResult};

/// Verify that `ydotool` is invocable and `ydotoold` is running. Called
/// once at startup of `listen` mode so the user gets a clear error
/// instead of a silent paste failure later.
pub fn verify_available() -> AppResult<()> {
    let output = Command::new("ydotool")
        .arg("--version")
        .output()
        .map_err(|e| AppError::YdotoolMissing(format!("`ydotool --version` failed: {e}")))?;
    if !output.status.success() {
        return Err(AppError::YdotoolMissing(format!(
            "ydotool exited with status {}; install via scripts/install-ydotool.sh",
            output.status
        )));
    }
    tracing::info!(
        version = %String::from_utf8_lossy(&output.stdout).trim(),
        "ydotool available"
    );
    Ok(())
}

/// Paste `text` into whatever app currently has keyboard focus:
/// 1. Snapshot the current clipboard text (if any).
/// 2. Write `text` to the clipboard.
/// 3. Invoke `ydotool key 29:1 47:1 47:0 29:0` (Ctrl-down V-down V-up Ctrl-up).
/// 4. Sleep 500 ms so the paste latches.
/// 5. Restore the original clipboard.
///
/// Blocking by design — the caller (Phase 2 `run_listen` release handler)
/// runs this from `tokio::task::spawn_blocking` or a sync section so the
/// async runtime isn't stalled.
pub fn inject_text(text: &str) -> AppResult<()> {
    if text.is_empty() {
        return Ok(());
    }

    let saved = snapshot_clipboard().ok();

    write_clipboard(text)?;

    let status = Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"])
        .status()
        .map_err(|e| AppError::YdotoolMissing(format!("running ydotool key: {e}")))?;
    if !status.success() {
        return Err(AppError::YdotoolMissing(format!(
            "ydotool key exited with status {status}"
        )));
    }

    std::thread::sleep(Duration::from_millis(500));

    if let Some(original) = saved {
        // Best-effort restore; log on failure but don't propagate.
        if let Err(e) = write_clipboard(&original) {
            tracing::warn!(error = %e, "failed to restore original clipboard");
        }
    }

    Ok(())
}

fn snapshot_clipboard() -> AppResult<String> {
    let (mut pipe, _mime) =
        get_contents(ClipboardType::Regular, PasteSeat::Unspecified, PasteMime::Text)
            .map_err(|e| AppError::Config(format!("clipboard snapshot: {e}")))?;
    let mut buf = String::new();
    pipe.read_to_string(&mut buf)
        .map_err(AppError::Io)?;
    Ok(buf)
}

fn write_clipboard(text: &str) -> AppResult<()> {
    let mut opts = CopyOptions::new();
    opts.foreground(false); // serve in a forked process so we don't block
    opts.copy(
        Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
        CopyMime::Specific("text/plain;charset=utf-8".into()),
    )
    .map_err(|e| AppError::Config(format!("clipboard write: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_empty_text_is_noop() {
        // Even without ydotool/Wayland available, the empty-text early return
        // must not call any external command.
        assert!(inject_text("").is_ok());
    }

    /// Verify the public function signature is `pub fn verify_available() -> AppResult<()>`.
    /// Real exec is gated behind #[ignore] since not all environments have ydotool.
    #[test]
    #[ignore]
    fn verify_available_when_ydotool_present() {
        // Run with: cargo test --lib injector -- --ignored
        verify_available().expect("ydotool should be available");
    }
}
```

**IMPORTANT** about API drift on wl-clipboard-rs 0.9:
- `wl_clipboard_rs::paste::get_contents` and `wl_clipboard_rs::copy::{Options, Source, MimeType}` — these are stable across recent versions, but if the implementer hits build errors mentioning these symbols, STOP and report `NEEDS_CONTEXT` with the actual signatures from `~/.cargo/registry/src/index.crates.io-*/wl-clipboard-rs-0.9*/src/`.

- [ ] **Step 3: Build and run tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib injector 2>&1 | tail -5
```

Expected: clean build; 1 active test passes (`inject_empty_text_is_noop`), 1 ignored.

- [ ] **Step 4: If `ydotool` is installed (it should be after install-ydotool.sh from Task 2.6), run the ignored test:**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib injector -- --ignored 2>&1 | tail -5
```

This is OK to skip if ydotool isn't installed yet — the install-ydotool.sh script in Task 2.6 sets it up. Run this verification ONLY after Task 2.6 is done.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/injector.rs linux/src/lib.rs
git commit -m "feat(linux): add clipboard + ydotool injector"
```

---

## Task 2.5: Wire `run_listen` — hotkey loop + paste handler

**Files:**
- Modify: `linux/src/main.rs`

Replace the placeholder `run_listen` with the full implementation.

- [ ] **Step 1: Find the placeholder in `linux/src/main.rs`:**

```rust
fn run_listen(_cfg: Config) -> anyhow::Result<()> {
    // Implemented in Task 2.5. For now, just print a placeholder so the CLI
    // dispatch is testable end-to-end.
    println!("listen subcommand: hotkey + paste wiring not yet implemented (Task 2.5)");
    Ok(())
}
```

Replace ENTIRELY with:

```rust
fn run_listen(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg.resolve_model_path().context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting listen mode");

    voice_input::injector::verify_available()
        .context("ydotool must be installed and ydotoold running")?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(run_listen_async(cfg, model_path))
}

async fn run_listen_async(cfg: Config, model_path: std::path::PathBuf) -> anyhow::Result<()> {
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
                    // The portal occasionally double-emits activated; ignore if already recording.
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, language_hint.clone()) {
                    Ok(p) => current_pipeline = Some(p),
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
            }
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    // drain_and_join is blocking (joins worker threads, ~50 ms typical);
                    // run on the blocking pool so the async runtime isn't stalled.
                    let segments = tokio::task::spawn_blocking(move || pipeline.drain_and_join())
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
            else => {
                // All streams closed (portal disconnected). Exit cleanly.
                tracing::warn!("portal streams closed; exiting");
                break;
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Verify build**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
```

Expected: clean. If you see warnings about unused imports or futures-util's `StreamExt` not being recognized, double-check the `use futures_util::stream::StreamExt;` line.

- [ ] **Step 3: Smoke test the early-error paths (no ydotool):**

```bash
# Temporarily hide ydotool to verify the error message is clear
if command -v ydotool >/dev/null; then
  HIDE_DIR="$(mktemp -d)"
  cp "$(command -v ydotool)" "$HIDE_DIR/ydotool.bak"
  PATH_BEFORE="$PATH"
  PATH="$(echo "$PATH" | tr ':' '\n' | grep -v "$(dirname "$(command -v ydotool)")" | tr '\n' ':')"
  cd /home/desmond/Repos/voice-input-src/linux
  PATH="$HOME/.cargo/bin:$PATH" cargo run -- listen 2>&1 | head -10 || true
  export PATH="$PATH_BEFORE"
fi
```

Expected: `Error: ydotool must be installed and ydotoold running` followed by `Caused by: ydotool unavailable: ...`. Non-zero exit.

If `ydotool` is not yet installed on the system, the same error message appears without any setup — that's the expected baseline before Task 2.6.

- [ ] **Step 4: Run all tests, confirm no regressions**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: all prior tests still pass.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/main.rs
git commit -m "feat(linux): wire hotkey-driven listen mode with portal + paste"
```

---

## Task 2.6: install-ydotool.sh + README updates

**Files:**
- Create: `linux/scripts/install-ydotool.sh`
- Modify: `linux/README.md`

- [ ] **Step 1: Create `linux/scripts/install-ydotool.sh`** with this exact content (and make it executable):

```bash
#!/bin/bash
# Bootstrap ydotool for Phase 2 listen mode.
#
# Installs the ydotool binary, sets up the /dev/uinput permissions via udev,
# adds the current user to the `input` group, and enables a systemd user
# service for the ydotoold daemon.
#
# Run once after installing the voice-input binary. Requires sudo for the
# apt install + udev rule + group add steps. The systemd user service is
# installed without sudo into ~/.config/systemd/user/.
#
# After this script, log out and log back in for the input-group membership
# to take effect, then verify with:
#   ydotool key 28:1 28:0
# (which simulates the Enter key — useful for a no-op test).

set -euo pipefail

if ! command -v sudo >/dev/null; then
  echo "Error: sudo not found; this script needs sudo to install packages and set udev rules."
  exit 1
fi

echo ">>> Installing ydotool via apt..."
sudo apt-get update
sudo apt-get install -y ydotool

echo ">>> Installing udev rule for /dev/uinput..."
sudo tee /etc/udev/rules.d/80-uinput.rules > /dev/null <<'EOF'
KERNEL=="uinput", GROUP="input", MODE="0660"
EOF
sudo udevadm control --reload-rules
sudo udevadm trigger

echo ">>> Adding $USER to the 'input' group..."
sudo usermod -aG input "$USER"

echo ">>> Installing systemd --user unit for ydotoold..."
mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/ydotoold.service" <<'EOF'
[Unit]
Description=ydotool daemon
After=default.target

[Service]
Type=simple
ExecStart=/usr/bin/ydotoold
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now ydotoold

echo
echo "============================================================"
echo "ydotool installed."
echo
echo "IMPORTANT: log out and log back in for input-group membership"
echo "to take effect. Verify with:"
echo
echo "  groups | grep input"
echo "  systemctl --user status ydotoold"
echo "  ydotool key 28:1 28:0   # simulates the Enter key"
echo "============================================================"
```

Make it executable:

```bash
chmod +x /home/desmond/Repos/voice-input-src/linux/scripts/install-ydotool.sh
```

- [ ] **Step 2: Verify the script has the executable bit**

```bash
ls -l /home/desmond/Repos/voice-input-src/linux/scripts/install-ydotool.sh
```

Expected: `-rwxrwxr-x` (the `x` bit must be set for owner+group at minimum).

- [ ] **Step 3: Rewrite `linux/README.md`** — replace the entire file with this content:

````markdown
# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **Phase 2** — hotkey-driven dictation works end-to-end via the XDG portal + ydotool. Tray mode (default invocation) and transcribe CLI mode (Phase 1) both still work.

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

## Compositor support

- **KDE Plasma 6**: target compositor, built-in StatusNotifierItem host. Portal `GlobalShortcuts` works out of the box.
- **sway**: requires waybar with `tray` module. Portal support via `xdg-desktop-portal-wlr` (less mature than KDE; document a compositor-binding fallback if portal proves unreliable).
- **hyprland**: requires waybar / ironbar / Riftbar with `tray` module. Portal support via `xdg-desktop-portal-hyprland`.
- **GNOME**: **not supported.** Mutter lacks `wlr-layer-shell` (needed in Phase 3).

## Config

`~/.config/voice-input/config.toml` — created on first run. Edit and restart to change. Notable keys:

- `language_hint = "zh"` — passed to whisper as a hint (`"en"`, `"ja"`, etc., or empty for auto-detect)
- `whisper_model_size = "small"` — determines the default model path
- `shortcut_handle` — populated by the portal on first listen-mode run; leave alone

## Project layout

See `../plans/voice-input-linux.md` for the full design and `../implementation/` for per-phase implementation plans.
````

- [ ] **Step 4: Verify README**

```bash
wc -l /home/desmond/Repos/voice-input-src/linux/README.md
head -1 /home/desmond/Repos/voice-input-src/linux/README.md
grep -c '^```' /home/desmond/Repos/voice-input-src/linux/README.md
```

Expected: ~90 lines, first line `# VoiceInput (Linux)`, even number of fences (each opening matched with closing).

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/scripts/install-ydotool.sh linux/README.md
git commit -m "docs(linux): add ydotool install script and listen-mode README section"
```

---

## Task 2.7: Manual end-to-end smoke test (user-driven)

This is the Phase 2 acceptance gate. The controller hands this to the user.

- [ ] **Step 1: Install ydotool**

```bash
cd /home/desmond/Repos/voice-input-src/linux
./scripts/install-ydotool.sh
# Then log out and log back in for input-group membership.
```

Verify after re-login:
```bash
groups | grep input               # should show "input"
systemctl --user status ydotoold  # should be active (running)
ydotool key 28:1 28:0             # silent — simulates Enter key
```

If `ydotool key` errors with "permission denied" on `/dev/uinput`, the udev rule didn't take effect — try `sudo udevadm trigger` and `systemctl --user restart ydotoold`.

- [ ] **Step 2: Run the ignored verify_available test now that ydotool is installed:**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib injector -- --ignored 2>&1 | tail -5
```

Expected: `verify_available_when_ydotool_present ... ok`.

- [ ] **Step 3: Build the release binary**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build --release
```

- [ ] **Step 4: Open a target application**

Open any text-editable GUI app (Kate, gedit, Firefox URL bar, a terminal with `cat > /tmp/out.txt`, etc.). Keep it visible alongside your shell.

- [ ] **Step 5: Run `listen` mode**

```bash
RUST_LOG=info ./target/release/voice-input listen
```

Expected log output:
```
INFO voice_input: config loaded ...
INFO voice_input::main: starting listen mode model=...
INFO voice_input::injector: ydotool available version=...
INFO voice_input::hotkey: portal bound shortcuts shortcuts=["toggle_recording"]
INFO voice_input::main: listen mode running — hold the bound shortcut to dictate; press Ctrl+C to exit
```

**On first run, the XDG portal pops up a dialog asking you to bind the "Hold to dictate" shortcut.** Bind it to **Right Ctrl** (or another single key you're comfortable with). Click Confirm.

If the portal dialog never appears, the portal backend isn't running. Verify with:
```bash
busctl --user list | grep -i portal
```
You should see `org.freedesktop.portal.Desktop` and at least one backend (`org.freedesktop.impl.portal.desktop.kde` for KDE Plasma).

- [ ] **Step 6: Focus the target app, hold Right Ctrl, speak, release**

While holding Right Ctrl, say: `"你好世界"` (or any short phrase). Release.

Expected (in voice-input log):
```
INFO voice_input::main: shortcut pressed; starting pipeline
... whisper init logs ...
INFO voice_input::main: listening — ...
INFO voice_input::main: shortcut released; draining and pasting
INFO voice_input::main: pasting segments=1 bytes=...
```

Expected (in the focused app): the text **"你好世界"** appears at the cursor position. The clipboard contents from before are restored ~500 ms after the paste — verify by pasting into a different field afterward; you should see whatever was originally on the clipboard (if anything).

- [ ] **Step 7: Repeat with English / mixed**

Hold Right Ctrl, say `"hello world"`, release → text "hello world" should appear in the target app.

Hold Right Ctrl, say `"Python 和 JSON"` (mixed), release → text appears (likely as Chinese homophones for Python/JSON; that's Phase 4 LLM refiner's job to fix).

- [ ] **Step 8: Test regression — Phase 0 tray + Phase 1 transcribe still work**

```bash
# Phase 0 tray
RUST_LOG=info ./target/release/voice-input
# Verify tray icon appears in system tray; right-click → Quit. Process exits.

# Phase 1 transcribe
RUST_LOG=info ./target/release/voice-input transcribe
# Speak a short phrase; verify [segment N] appears in stdout. Ctrl+C to stop.
```

Both should work exactly as in Phase 1.

- [ ] **Step 9: Report**

Phase 2 passes if:
- ✅ Portal binding dialog appeared on first listen-mode run (or shortcut was already bound from a prior run)
- ✅ Pressing the bound key starts the pipeline (log shows "shortcut pressed")
- ✅ Releasing the key pastes the transcribed text into the focused application
- ✅ Original clipboard contents are restored after paste
- ✅ Ctrl+C in the listen terminal cleanly exits
- ✅ Phase 0 tray and Phase 1 transcribe modes still work (no regression)

If any step fails, capture the log + error output for a fix subtask.

---

## Task 2.8: Final verification + push

- [ ] **Step 1: Run all tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: ≥30 passing (29 from Task 2.1 + 1 hotkey + 1 injector active = 31), 2 ignored (whisper inference + ydotool live).

- [ ] **Step 2: Release build clean**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build --release 2>&1 | grep -E "warning|error" | grep -v Compiling | head -20
```

Expected: no warnings or errors from `voice-input` crate.

- [ ] **Step 3: Format check**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt --check 2>&1
```

If the output is non-empty, apply formatting:
```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
cd /home/desmond/Repos/voice-input-src
git add -u linux/
git commit -m "style(linux): cargo fmt"
```

- [ ] **Step 4: Clippy**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -30
```

If clippy finds trivial issues (unused imports, needless borrows, etc.), apply fixes and commit:
```bash
git commit -m "chore(linux): fix clippy findings"
```

If clippy finds non-trivial issues (suggested refactors / API redesigns), STOP and report DONE_WITH_CONCERNS.

- [ ] **Step 5: Push the feature branch**

```bash
cd /home/desmond/Repos/voice-input-src
git push -u origin linux/phase-2-hotkey-paste 2>&1
```

Expected:
- `* [new branch]      linux/phase-2-hotkey-paste -> linux/phase-2-hotkey-paste`
- `branch 'linux/phase-2-hotkey-paste' set up to track 'origin/linux/phase-2-hotkey-paste'.`

If the controller forgot to create the feature branch beforehand and you're still on `main`, STOP and report — request a feature branch be created.

- [ ] **Step 6: Final verification**

```bash
git log origin/main..HEAD --oneline
git status
git branch -vv
```

Expected:
- Multiple commits from Tasks 2.1–2.8 (plus optional style/chore commits)
- Working tree clean
- Branch tracks `origin/linux/phase-2-hotkey-paste`

---

## Self-Review Notes

**Spec coverage** (from `plans/voice-input-linux.md` Phase 2):
- ✅ ashpd `GlobalShortcuts` press/release → Task 2.3
- ✅ `wl-clipboard-rs` snapshot/restore → Task 2.4
- ✅ `ydotool` shell-out → Task 2.4 + Task 2.6 install script
- ✅ "End-to-end: hold key, speak, release, transcript pastes into another window" → Task 2.5 + Task 2.7 verification
- ✅ Still CLI-only — no GUI overlay (that's Phase 3)
- ✅ Compositor-binding fallback documented in README (Task 2.6) — defer implementation to Phase 5 polish

**Phase 1 entry conditions addressed:**
1. ✅ `PipelineHandle::Drop` impl → Task 2.1
2. ✅ 3 unit tests for `resolve_model_path` → Task 2.1
3. ✅ `drain_and_join` method needed by Phase 2 → Task 2.1
4. ✅ Threading model documented as runtime-agnostic (Phase 1) holds — Phase 2 only adds an async layer in `main.rs`, doesn't touch pipeline modules

**Placeholder scan:** no "TBD", "TODO", or "fill in details" in steps. Every code block is complete.

**Type consistency check:**
- `HotkeyHandle::create()` returns `AppResult<HotkeyHandle<'a>>` — used in `run_listen_async` ✓
- `hotkey.activated().await` / `hotkey.deactivated().await` return streams — `StreamExt::next()` used in select! ✓
- `PipelineHandle::drain_and_join(self) -> Vec<String>` — consumed in `spawn_blocking` ✓
- `injector::verify_available()` and `injector::inject_text(&str)` — both used in main.rs ✓
- `SHORTCUT_ID = "toggle_recording"` — defined in hotkey.rs, used by ashpd binding ✓
- `NewShortcut::new(SHORTCUT_ID, "Hold to dictate").preferred_trigger("CTRL_R")` — assumed ashpd 0.13 builder API; subagent STOPs on drift

**Scope check:** Phase 2 deliberately excludes the overlay window (Phase 3), LLM refiner (Phase 4), Settings/menus (Phase 5), first-run wizard (Phase 6), packaging (Phase 7+). These are explicit non-goals.

**Known risks:**
- `ashpd 0.13` API drift — instruction to STOP and report NEEDS_CONTEXT in Task 2.3 + 2.4 if signatures differ.
- `xdg-desktop-portal-wlr` (sway) global-shortcuts implementation is less mature than KDE. Manual smoke test in Task 2.7 happens on KDE Plasma 6 which is the most-mature backend. If sway support is sub-par, document and defer compositor-binding fallback to Phase 5.
- `ydotool key 29:1 47:1 47:0 29:0` is the Linux evdev keycode for Ctrl+V. If a different keyboard layout makes V map to a different keycode... actually no, evdev keycodes are layout-independent (47 is always the physical V position). Should work on any layout.
- Clipboard race: the receiving application has 500 ms to consume the clipboard before we restore. For most apps this is plenty; slow apps (e.g., Electron startup) may miss the paste. If reports come in, the 500 ms can be made configurable in Phase 5 Settings.
