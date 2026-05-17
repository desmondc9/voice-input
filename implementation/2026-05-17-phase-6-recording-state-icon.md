# Phase 6: Recording-State Tray Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The system-tray icon switches from the idle microphone glyph to a red "recording" indicator while the speech pipeline is active, mirroring macOS's `mic` ↔ `mic.fill`/`systemRed` behavior.

**Architecture:** Add a shared `Arc<AtomicBool>` to `AppState` that the tray's `icon_name()` method reads. The backend listen loop sets the flag to `true` after `speech::start_pipeline` succeeds and back to `false` after the drain completes. ksni's `Handle::update()` is called whenever the flag flips so the StatusNotifierItem D-Bus signal fires and the tray host re-fetches `icon_name`. No new dependencies — `AtomicBool` is in std and `ksni` is already a dep.

**Tech Stack:** ksni 0.3 (existing), std::sync::atomic (stdlib).

---

## Pre-flight: Phase 5 entry conditions

Verify before starting:

- `main` at `70b89c1` (Phase 5 merged + Test-button fix)
- 49 tests pass, clippy clean
- Smoke test acceptance: tray menu, Enabled toggle, Language switch, Settings dialog (open + Test + Save), Quit all confirmed working
- `Config.enabled` exists; `AppState` exposes `shutdown` + `config_changed` Notify channels

Branch:

```bash
cd /home/desmond/Repos/voice-input-src
git checkout main
git pull --ff-only
git checkout -b linux/phase-6-recording-state-icon
```

---

## File structure

```
linux/
  src/
    state.rs          # ADD `recording: Arc<AtomicBool>` field + tests
    tray.rs           # MODIFY icon_name() to branch on state.recording
    main.rs           # MODIFY run_backend_async to flip flag + call handle.update()
    README.md         # UPDATE Status block to mention recording icon
```

No new files. No new dependencies.

---

## Task 6.1: Add `recording: Arc<AtomicBool>` to AppState

**Files:**
- Modify: `linux/src/state.rs`

The flag lives on AppState (not inside VoiceInputTray) because both the tray-reader path (icon_name) and the backend-writer path (pipeline lifecycle) need access, and AppState is already cloneable + shared.

- [ ] **Step 1: Write the failing test**

Append this to the existing `mod tests` block in `linux/src/state.rs`:

```rust
    #[test]
    fn recording_flag_defaults_to_false() {
        let state = AppState::new(Config::default());
        assert!(
            !state.recording.load(std::sync::atomic::Ordering::Acquire),
            "fresh AppState must not report 'recording'"
        );
    }

    #[test]
    fn recording_flag_is_shared_across_clones() {
        let a = AppState::new(Config::default());
        let b = a.clone();
        a.recording
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(
            b.recording.load(std::sync::atomic::Ordering::Acquire),
            "clones must share the same AtomicBool via Arc"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib state 2>&1 | tail -15
```

Expected: FAIL — compiler error `no field 'recording' on type 'AppState'`.

- [ ] **Step 3: Add the field + initializer**

In `linux/src/state.rs`:

1. Add to the imports block (at the top, after `use std::sync::Arc;`):

```rust
use std::sync::atomic::AtomicBool;
```

2. Add the field to the `AppState` struct definition. Replace:

```rust
#[derive(Clone)]
pub struct AppState {
    config: Arc<Mutex<Config>>,
    pub shutdown: Arc<Notify>,
    pub config_changed: Arc<Notify>,
}
```

with:

```rust
#[derive(Clone)]
pub struct AppState {
    config: Arc<Mutex<Config>>,
    pub shutdown: Arc<Notify>,
    pub config_changed: Arc<Notify>,
    /// True while a speech pipeline is active. Read by the tray to
    /// switch icon; written by the backend listen loop on pipeline
    /// start/end. Use Release on the write, Acquire on the read.
    pub recording: Arc<AtomicBool>,
}
```

3. Initialize the field in `AppState::new`. Replace:

```rust
    pub fn new(cfg: Config) -> Self {
        Self {
            config: Arc::new(Mutex::new(cfg)),
            shutdown: Arc::new(Notify::new()),
            config_changed: Arc::new(Notify::new()),
        }
    }
```

with:

```rust
    pub fn new(cfg: Config) -> Self {
        Self {
            config: Arc::new(Mutex::new(cfg)),
            shutdown: Arc::new(Notify::new()),
            config_changed: Arc::new(Notify::new()),
            recording: Arc::new(AtomicBool::new(false)),
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib state 2>&1 | tail -10
```

Expected: PASS — 4 tests in `state` module (2 pre-existing + 2 new).

- [ ] **Step 5: Build the whole crate to catch any unused-import warnings**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: both clean.

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/state.rs
git commit -m "feat(linux): AppState.recording flag (Arc<AtomicBool>)"
```

---

## Task 6.2: Tray `icon_name()` reads recording state

**Files:**
- Modify: `linux/src/tray.rs`

When the flag flips, ksni will re-fetch `icon_name()` (triggered in Task 6.3 by `Handle::update`). The recording icon is `media-record-symbolic` — present in Adwaita/Breeze and almost every other freedesktop icon theme, and the "red dot" semantic is universally understood as "recording".

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests` block at the bottom of `linux/src/tray.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::overlay;
    use std::sync::atomic::Ordering;

    use ksni::Tray as _;

    #[test]
    fn icon_reflects_recording_state() {
        let state = crate::state::AppState::new(Config::default());
        let (ui_tx, _ui_rx) = overlay::channel();
        let tray = VoiceInputTray::new(state.clone(), ui_tx);

        assert_eq!(tray.icon_name(), IDLE_ICON, "idle by default");

        state.recording.store(true, Ordering::Release);
        assert_eq!(
            tray.icon_name(),
            RECORDING_ICON,
            "recording flag flips icon"
        );

        state.recording.store(false, Ordering::Release);
        assert_eq!(tray.icon_name(), IDLE_ICON, "flag clears back to idle");
    }
}
```

If `tray.rs` already has a `#[cfg(test)] mod tests` block, append the new test inside it instead of declaring a second `mod tests`.

- [ ] **Step 2: Run test to verify it fails**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib tray 2>&1 | tail -15
```

Expected: FAIL — `IDLE_ICON` / `RECORDING_ICON` not defined.

- [ ] **Step 3: Add icon-name constants and update `icon_name()`**

At the top of `linux/src/tray.rs`, just after the imports, add:

```rust
/// Tray icon when no pipeline is active. Standard freedesktop icon name
/// present in Adwaita / Breeze / Yaru / Papirus.
pub(crate) const IDLE_ICON: &str = "audio-input-microphone";

/// Tray icon while a pipeline is active. The "red dot" record indicator —
/// universally rendered in red across mainstream icon themes.
pub(crate) const RECORDING_ICON: &str = "media-record-symbolic";
```

Then replace the existing `icon_name` implementation:

```rust
    fn icon_name(&self) -> String { "audio-input-microphone".into() }
```

with the state-aware version:

```rust
    fn icon_name(&self) -> String {
        if self
            .state
            .recording
            .load(std::sync::atomic::Ordering::Acquire)
        {
            RECORDING_ICON.into()
        } else {
            IDLE_ICON.into()
        }
    }
```

Also update `tool_tip().icon_name` (same field is set there too) to use `IDLE_ICON` as the static tooltip icon. Replace:

```rust
            icon_name: "audio-input-microphone".into(),
```

inside `tool_tip()` with:

```rust
            icon_name: IDLE_ICON.into(),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib tray 2>&1 | tail -10
```

Expected: PASS — new test plus whatever was already there.

- [ ] **Step 5: Whole-crate build + clippy**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: both clean.

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/tray.rs
git commit -m "feat(linux): tray icon switches to media-record-symbolic while recording"
```

---

## Task 6.3: Wire pipeline lifecycle to flip the flag + refresh tray

**Files:**
- Modify: `linux/src/main.rs`

The backend `run_backend_async` is where `Activated` / `Deactivated` portal events are handled. After a successful `speech::start_pipeline`, store `true` and call `tray_handle.update(|_| {}).await` so ksni emits the D-Bus signal that re-renders the tray. After drain completes, store `false` + update again. ksni 0.3's `Handle::update` takes a closure receiving `&mut Self` — we don't need to mutate the tray struct itself (state lives in AppState), so we pass `|_| {}`.

- [ ] **Step 1: Keep the tray handle**

In `linux/src/main.rs`, the current line in `run_backend_async` discards the handle:

```rust
    let _tray_handle = tray.spawn().await.context("spawning tray")?;
```

Replace with:

```rust
    let tray_handle = tray.spawn().await.context("spawning tray")?;
```

- [ ] **Step 2: Flip the flag + refresh on pipeline START**

Find the `Activated` arm. The current success branch reads:

```rust
                match speech::start_pipeline(&model_path, snap.language_hint.clone(), Some(level_tx.clone())) {
                    Ok((capture, p)) => {
                        let _ = overlay_tx.send(UiCmd::Show);
                        current_capture = Some(capture);
                        current_pipeline = Some(p);
                    }
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
```

Replace with:

```rust
                match speech::start_pipeline(&model_path, snap.language_hint.clone(), Some(level_tx.clone())) {
                    Ok((capture, p)) => {
                        let _ = overlay_tx.send(UiCmd::Show);
                        current_capture = Some(capture);
                        current_pipeline = Some(p);
                        state
                            .recording
                            .store(true, std::sync::atomic::Ordering::Release);
                        tray_handle.update(|_| {}).await;
                        tracing::info!("tray: icon → recording");
                    }
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
```

- [ ] **Step 3: Flip the flag + refresh on pipeline END**

Find the `Deactivated` arm. After the existing drain/paste block, before `let _ = overlay_tx.send(UiCmd::Hide);`, add the flag-clear + tray refresh. Replace the end of the arm:

```rust
                    let _ = overlay_tx.send(UiCmd::Hide);
                }
            }
```

with:

```rust
                    let _ = overlay_tx.send(UiCmd::Hide);
                    state
                        .recording
                        .store(false, std::sync::atomic::Ordering::Release);
                    tray_handle.update(|_| {}).await;
                    tracing::info!("tray: icon → idle");
                }
            }
```

Note: the flag-clear happens **after** the paste finishes. If you prefer the icon to clear the moment recording stops (audio capture released) rather than when paste finishes, move the clear to immediately after `drop(current_capture.take())`. Default: clear-after-paste matches macOS, which keeps the red mic up while the paste is happening.

- [ ] **Step 4: Build + test**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: build OK, all tests pass, clippy clean.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/main.rs
git commit -m "feat(linux): flip tray icon on pipeline start/end via Handle::update"
```

---

## Task 6.4: README + smoke test + final verification + push

**Files:**
- Modify: `linux/README.md`

- [ ] **Step 1: Update README Status block**

In `linux/README.md` line 5, the current Status line reads:

```markdown
> Status: **Phase 5** — tray menu (Enabled / Language / LLM Refinement → Settings dialog) replaces manual TOML editing. ...
```

Replace it with:

```markdown
> Status: **Phase 6** — tray icon now reflects pipeline state (`media-record-symbolic` while dictating, `audio-input-microphone` idle), matching the macOS `mic`/`mic.fill` parity. Phase 5 features (Settings dialog, Enabled/Language/LLM submenus, unified default mode) remain. Headless `transcribe` CLI still works.
```

Also append a short paragraph to the "Run" / overlay description section explaining the icon behavior. Find the existing line that describes the overlay capsule (around line 87 in current README) and insert this sentence after it:

```markdown

The tray icon switches to a red **record** glyph (`media-record-symbolic`) for the entire duration of dictation — from the moment the pipeline starts to when the refined text is pasted — and reverts to the microphone glyph afterwards. This mirrors the macOS app's behavior.
```

- [ ] **Step 2: cargo fmt**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
PATH="$HOME/.cargo/bin:$PATH" cargo fmt -- --check 2>&1 | head -5
```

Expected: no diff output.

- [ ] **Step 3: Final test + clippy sanity**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: 51 tests pass (49 from Phase 5 + 2 new in state + 1 new in tray), clippy clean.

- [ ] **Step 4: User smoke test**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build --release
RUST_LOG=info ./target/release/voice-input
```

Acceptance checklist (have a human watch the tray):

1. After launch, tray icon shows a microphone glyph (idle).
2. Hold Ctrl+Space → tray icon switches to a red dot (record) glyph immediately.
3. Release Ctrl+Space → icon stays red until the paste happens, then reverts to microphone.
4. Log contains the two new lines on each dictation cycle:
   - `tray: icon → recording`
   - `tray: icon → idle`
5. Toggling Enabled off → pressing hotkey logs the existing `Enabled=false; ignoring` line and the icon stays microphone (the flag is never set).
6. Quit from tray exits cleanly.

If the recording icon does NOT visibly change in your icon theme, try one of the fallback icon names below (edit `RECORDING_ICON` in `tray.rs`):
- `media-record` (non-symbolic; some themes only ship one)
- `audio-input-microphone-high-symbolic` (volume-bar variant — subtler but always present)

- [ ] **Step 5: Push**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/README.md
git commit -m "docs(linux): describe Phase 6 recording-state tray icon"
git push -u origin linux/phase-6-recording-state-icon
```

- [ ] **Step 6: Merge to main**

```bash
git checkout main
git merge --ff-only linux/phase-6-recording-state-icon
git push origin main
```

---

## Self-Review Notes

**Spec coverage:**
- IDLE → RECORDING transition on pipeline start → Task 6.3 Step 2
- RECORDING → IDLE transition on pipeline end → Task 6.3 Step 3
- Tray re-render trigger → `Handle::update` called after each store
- State shared between tray reader + backend writer → AppState in Task 6.1
- macOS-parity intent (mic ↔ mic.fill + red) → `media-record-symbolic` is the closest freedesktop equivalent; documented in Task 6.4 with theme-specific fallbacks
- Tests covering the new logic → Task 6.1 (state) + Task 6.2 (icon_name branches)

**Architectural decisions:**
- AtomicBool over a third Notify channel: tray reads on every D-Bus query (could be many times per second); polling an atomic is essentially free, whereas hooking up another Notify subscription would mean wiring a stream inside the tray. Single writer (backend listen loop), many readers, plain Release/Acquire is sufficient — no ABA risk.
- `Handle::update(|_| {})`: ksni 0.3's API requires a closure mutating the tray, but our state lives outside, so the closure is a no-op. The `update` call's effect is "emit the D-Bus PropertiesChanged signal so the watcher re-fetches icon_name". This is documented behavior, not a workaround.
- Recording flag cleared **after** paste (in Deactivated arm): matches macOS UX where the user sees the recording state through "speech captured + transcribed + pasted", not just "I stopped talking".

**Known risks:**
- Icon theme variance: `media-record-symbolic` is in Adwaita/Breeze/Yaru/Papirus but not 100% of themes. Task 6.4 Step 4 includes fallback names if user reports invisible/identical icons.
- ksni 0.3 Handle::update is async — calling it in the tokio `select!` arm is fine, but if the D-Bus connection is wedged, the await could block. Acceptable: if ksni is broken, the whole tray is broken anyway, and the user would see no tray menu either.
- A pipeline-start failure (start_pipeline returns Err) does NOT set recording=true (the store is inside the Ok arm). Correct — no need to flip the icon to record state when nothing started.
- If the user holds the hotkey and the pipeline is already running (`current_pipeline.is_some()` short-circuit at the top of the Activated arm), the icon stays in its current state. Correct — recording is already true.

**Out of scope (deferred):**
- Tinting/coloring the existing microphone glyph instead of swapping icons (would need ksni `icon_pixmap` with a custom RGBA blit; not worth the complexity for Phase 6).
- A third "refining" state with its own icon while the LLM call is in flight. Would require a fourth icon and timing between Deactivated's drain and refine call. Could be Phase 6.5 if user asks.
