# Phase 5: Settings + Tray Menus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace manual TOML editing with a tray-menu UX matching macOS (Enabled toggle, Language submenu, LLM Refinement submenu with Settings dialog) and unify the tray + listen + overlay flows into one default mode launched by `voice-input` (no subcommand).

**Architecture:** Introduce `AppState` (an `Arc<Mutex<Config>>` + `Notify` channels) shared between the tray, the listen loop (rebuilds the refiner on Settings save), and the GTK Settings dialog. Rename `OverlayCmd` to `UiCmd` and add `OpenSettings` so the tray can ask the GTK main loop to build the dialog. Replace `run_tray` with a unified `run_app` that spawns the tokio backend (tray + listen + portal) on a std::thread and runs `gtk::Application::run_with_args(&[])` on the OS main thread.

**Tech Stack:** ksni 0.3 (existing), gtk4 0.10 (existing), parking_lot 0.12 (new sync mutex).

---

## Pre-flight: Phase 4 entry conditions

Verify before starting:

- `main` at `69f36e8`
- 47 tests pass
- `Config.llm_timeout_secs` exists (Phase 4)
- `LlmRefiner::try_refine` exists (Phase 4)

Branch:

```bash
cd /home/desmond/Repos/voice-input-src
git checkout main
git pull --ff-only
git checkout -b linux/phase-5-settings-tray-menus
```

---

## File structure

```
linux/
  Cargo.toml                    # add parking_lot
  src/
    state.rs                    # NEW - AppState (Arc<Mutex<Config>> + Notify)
    settings_window.rs          # NEW - GTK Settings dialog
    tray.rs                     # REWRITE - Enabled/Language/LLM submenu + Quit
    overlay/mod.rs              # rename OverlayCmd to UiCmd, add OpenSettings
    config.rs                   # add `enabled: bool` field
    lib.rs                      # add `pub mod state;` + `pub mod settings_window;`
    main.rs                     # delete run_tray; add run_app (unified)
```

---

## Task 5.1: Add `enabled` field to Config

**Files:** Modify `linux/src/config.rs`.

The struct already has `#[serde(default)]` (Phase 4) so missing-field legacy configs grandfather to the default.

**Step 1:** Add field via python heredoc.

```bash
python3 - <<'PY'
p = "/home/desmond/Repos/voice-input-src/linux/src/config.rs"
s = open(p).read()

old_field = """pub struct Config {
    pub language_hint: String,"""
new_field = """pub struct Config {
    /// Master switch. When false, the hotkey is observed but ignored.
    /// Mirrors macOS Enabled menu item.
    pub enabled: bool,
    pub language_hint: String,"""
assert old_field in s
s = s.replace(old_field, new_field, 1)

old_default = """        Self {
            language_hint: \"zh\".to_string(),"""
new_default = """        Self {
            enabled: true,
            language_hint: \"zh\".to_string(),"""
assert old_default in s
s = s.replace(old_default, new_default, 1)

old_test = """    fn defaults_have_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.language_hint, \"zh\");"""
new_test = """    fn defaults_have_sensible_values() {
        let cfg = Config::default();
        assert!(cfg.enabled, \"enabled defaults to true\");
        assert_eq!(cfg.language_hint, \"zh\");"""
assert old_test in s
s = s.replace(old_test, new_test, 1)

open(p, "w").write(s)
print("ok")
PY
```

**Step 2:** Test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib config 2>&1 | tail -10
cd /home/desmond/Repos/voice-input-src
git add linux/src/config.rs
git commit -m "feat(linux): add Config.enabled master switch"
```

---

## Task 5.2: Rename OverlayCmd to UiCmd, add OpenSettings

**Files:** Modify `linux/src/overlay/mod.rs`, `linux/src/main.rs`.

**Step 1:** Rewrite overlay/mod.rs to rename the enum + add OpenSettings variant.

The new overlay/mod.rs contents (paste exactly):

```rust
//! GTK4 + layer-shell overlay capsule shown during listen mode.

pub mod waveform;
pub mod window;

pub use window::OverlayWindow;

use std::sync::mpsc;

/// Commands the backend thread sends to the GTK main thread.
#[derive(Debug, Clone)]
pub enum UiCmd {
    Show,
    SetLevel(f32),
    SetText(String),
    Hide,
    Quit,
    /// Tray Settings... clicked. GTK builds or re-presents the Settings dialog.
    OpenSettings,
}

pub type UiSender = mpsc::Sender<UiCmd>;
pub type UiReceiver = mpsc::Receiver<UiCmd>;

pub fn channel() -> (UiSender, UiReceiver) {
    mpsc::channel()
}
```

**Step 2:** Update main.rs references via sed:

```bash
cd /home/desmond/Repos/voice-input-src/linux
sed -i \
    -e "s/OverlayCmd/UiCmd/g" \
    -e "s/OverlaySender/UiSender/g" \
    -e "s/OverlayReceiver/UiReceiver/g" \
    src/main.rs
```

**Step 3:** Build + test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
cd /home/desmond/Repos/voice-input-src
git add linux/src/overlay/mod.rs linux/src/main.rs
git commit -m "refactor(linux): rename OverlayCmd to UiCmd and add OpenSettings variant"
```

---

## Task 5.3: parking_lot dep + AppState module

**Files:**
- Modify `linux/Cargo.toml` (add parking_lot 0.12)
- Create `linux/src/state.rs`
- Create stub `linux/src/settings_window.rs` (filled in Task 5.7)
- Modify `linux/src/lib.rs` to register both modules

**Step 1:** Add parking_lot in Cargo.toml between `ksni` and `reqwest` (alphabetical).

**Step 2:** Create `linux/src/state.rs` with this content:

```rust
//! Shared application state across tray, listen loop, and Settings dialog.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct AppState {
    config: Arc<Mutex<Config>>,
    pub shutdown: Arc<Notify>,
    pub config_changed: Arc<Notify>,
}

impl AppState {
    pub fn new(cfg: Config) -> Self {
        Self {
            config: Arc::new(Mutex::new(cfg)),
            shutdown: Arc::new(Notify::new()),
            config_changed: Arc::new(Notify::new()),
        }
    }

    pub fn snapshot(&self) -> Config {
        self.config.lock().clone()
    }

    /// Mutate the config and persist to disk. Notifies config_changed on
    /// success so the listen loop can rebuild the refiner.
    pub fn update<F>(&self, mutator: F) -> AppResult<()>
    where
        F: FnOnce(&mut Config),
    {
        let snapshot = {
            let mut guard = self.config.lock();
            mutator(&mut guard);
            guard.clone()
        };
        snapshot
            .save()
            .map_err(|e| AppError::Config(format!("persist on update: {}", e)))?;
        self.config_changed.notify_waiters();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_current_state() {
        let cfg = Config {
            language_hint: "ja".into(),
            ..Config::default()
        };
        let state = AppState::new(cfg);
        assert_eq!(state.snapshot().language_hint, "ja");
    }

    #[test]
    fn direct_mutex_mutation_visible_in_snapshot() {
        let state = AppState::new(Config::default());
        {
            let mut guard = state.config.lock();
            guard.enabled = false;
        }
        assert!(!state.snapshot().enabled);
    }
}
```

**Step 3:** Create stub `linux/src/settings_window.rs`:

```rust
//! GTK Settings dialog - populated in Task 5.7.
```

**Step 4:** Register modules in lib.rs by inserting after `pub mod refiner;`:

```bash
sed -i "/^pub mod refiner;/a pub mod settings_window;\npub mod state;" linux/src/lib.rs
```

**Step 5:** Build + test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib state 2>&1 | tail -5
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/Cargo.lock linux/src/state.rs linux/src/settings_window.rs linux/src/lib.rs
git commit -m "feat(linux): add AppState (parking_lot Mutex<Config> + Notify channels)"
```

---

## Task 5.4: Tray Enabled toggle

**Files:** Modify `linux/src/tray.rs`.

Rewrite the tray to own `AppState` and add the Enabled checkbox. Subsequent tasks add Language + LLM submenus.

New tray.rs (paste exactly):

```rust
use ksni::{menu::CheckmarkItem, menu::StandardItem, MenuItem, Tray};

use crate::overlay::{UiCmd, UiSender};
use crate::state::AppState;

pub struct VoiceInputTray {
    pub state: AppState,
    pub ui_tx: UiSender,
}

impl VoiceInputTray {
    pub fn new(state: AppState, ui_tx: UiSender) -> Self {
        Self { state, ui_tx }
    }
}

impl Tray for VoiceInputTray {
    fn id(&self) -> String { "com.yetone.VoiceInput".into() }
    fn title(&self) -> String { "VoiceInput".into() }
    fn icon_name(&self) -> String { "audio-input-microphone".into() }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "VoiceInput".into(),
            description: "Hold the configured key to dictate".into(),
            icon_name: "audio-input-microphone".into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let snap = self.state.snapshot();

        vec![
            CheckmarkItem {
                label: "Enabled".into(),
                checked: snap.enabled,
                activate: Box::new(|this: &mut Self| {
                    let new_value = !this.state.snapshot().enabled;
                    if let Err(e) = this.state.update(|cfg| cfg.enabled = new_value) {
                        tracing::error!(error = %e, "tray: failed to persist Enabled");
                    } else {
                        tracing::info!(enabled = new_value, "tray: Enabled toggled");
                    }
                }),
                ..Default::default()
            }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    tracing::info!("tray: Quit selected");
                    this.state.shutdown.notify_waiters();
                    let _ = this.ui_tx.send(UiCmd::Quit);
                }),
                ..Default::default()
            }.into(),
        ]
    }
}
```

**Step 2:** Update run_tray_async in main.rs to take AppState + ui_tx (interim - replaced in Task 5.9):

```bash
python3 - <<'PY'
p = "/home/desmond/Repos/voice-input-src/linux/src/main.rs"
s = open(p).read()

old = """async fn run_tray_async() -> anyhow::Result<()> {
    let shutdown = Arc::new(Notify::new());
    let tray = VoiceInputTray::new(shutdown.clone());
    let _tray_handle = tray.spawn().await.context(\"spawning tray\")?;

    tracing::info!(\"voice-input running — Quit via tray icon or Ctrl+C\");

    tokio::select! {
        _ = shutdown.notified() => tracing::info!(\"tray Quit received\"),
        _ = tokio::signal::ctrl_c() => tracing::info!(\"SIGINT received\"),
    }

    tracing::info!(\"shutdown complete\");
    Ok(())
}"""

new = """async fn run_tray_async() -> anyhow::Result<()> {
    // Interim: superseded by run_app in Task 5.9. Construct a no-receiver
    // UiSender (sends dropped) and a synthetic AppState from defaults.
    let state = voice_input::state::AppState::new(Config::default());
    let (ui_tx, _ui_rx) = voice_input::overlay::channel();
    let tray = VoiceInputTray::new(state.clone(), ui_tx);
    let _tray_handle = tray.spawn().await.context(\"spawning tray\")?;

    tracing::info!(\"voice-input tray running — Quit via tray or Ctrl+C\");

    tokio::select! {
        _ = state.shutdown.notified() => tracing::info!(\"tray Quit received\"),
        _ = tokio::signal::ctrl_c() => tracing::info!(\"SIGINT received\"),
    }

    tracing::info!(\"shutdown complete\");
    Ok(())
}"""
assert old in s
s = s.replace(old, new, 1)
open(p, "w").write(s)
print("ok")
PY
```

**Step 3:** Build + test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
cd /home/desmond/Repos/voice-input-src
git add linux/src/tray.rs linux/src/main.rs
git commit -m "feat(linux): tray Enabled toggle wired to AppState"
```

---

## Task 5.5: Tray Language submenu

**Files:** Modify `linux/src/tray.rs`.

Add a Language submenu with 6 entries mapping to whisper.cpp ISO-639-1 codes. The listen loop reads the language from `AppState.snapshot()` on each pipeline start (wired in Task 5.9).

**Step 1:** Insert into the menu() vec between the Enabled checkmark and the Separator. Append a helper at the end of the file.

The helper:

```rust
const LANGUAGES: &[(&str, &str)] = &[
    ("Auto-detect", ""),
    ("English", "en"),
    ("Zhongwen", "zh"),
    ("Nihongo", "ja"),
    ("Hangugeo", "ko"),
    ("Espanol", "es"),
];

fn language_menu(current: &str) -> MenuItem<VoiceInputTray> {
    let submenu: Vec<MenuItem<VoiceInputTray>> = LANGUAGES
        .iter()
        .map(|(label, code)| {
            let code = (*code).to_string();
            let label = (*label).to_string();
            let checked = current == code;
            CheckmarkItem {
                label,
                checked,
                activate: Box::new(move |this: &mut VoiceInputTray| {
                    let code_clone = code.clone();
                    if let Err(e) = this.state.update(|cfg| cfg.language_hint = code_clone.clone()) {
                        tracing::error!(error = %e, "tray: failed to persist language");
                    } else {
                        tracing::info!(language = %code, "tray: language changed");
                    }
                }),
                ..Default::default()
            }
            .into()
        })
        .collect();

    ksni::menu::SubMenu {
        label: "Language".into(),
        submenu,
        ..Default::default()
    }
    .into()
}
```

Insert `language_menu(&snap.language_hint),` into the menu() vec between the Enabled CheckmarkItem and the Separator.

Note: the language labels shown above use romanization to keep the heredoc safe; when implementing, use the native scripts:
- "Zhongwen" -> "Chinese (Simplified+Traditional aware)" or just the Chinese word for "Chinese"
- "Nihongo" -> Japanese word for "Japanese"
- "Hangugeo" -> Korean word for "Korean"

(Implementer: copy the native script labels from `dist/Sources/VoiceInput/AppDelegate.swift:190-197` adapted to whisper codes.)

**Step 2:** Build + test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
cd /home/desmond/Repos/voice-input-src
git add linux/src/tray.rs
git commit -m "feat(linux): tray Language submenu (6 whisper codes + auto-detect)"
```

---

## Task 5.6: Tray LLM Refinement submenu

**Files:** Modify `linux/src/tray.rs`.

Add LLM Refinement submenu with Enabled checkbox + Separator + Settings... item.

**Step 1:** Insert `llm_menu(snap.llm_enabled),` into the menu vec right after `language_menu(...)`. Append this helper:

```rust
fn llm_menu(llm_enabled: bool) -> MenuItem<VoiceInputTray> {
    let submenu: Vec<MenuItem<VoiceInputTray>> = vec![
        CheckmarkItem {
            label: "Enabled".into(),
            checked: llm_enabled,
            activate: Box::new(|this: &mut VoiceInputTray| {
                let new_value = !this.state.snapshot().llm_enabled;
                if let Err(e) = this.state.update(|cfg| cfg.llm_enabled = new_value) {
                    tracing::error!(error = %e, "tray: failed to persist LLM enabled");
                } else {
                    tracing::info!(llm_enabled = new_value, "tray: LLM toggled");
                }
            }),
            ..Default::default()
        }.into(),
        MenuItem::Separator,
        StandardItem {
            label: "Settings...".into(),
            activate: Box::new(|this: &mut VoiceInputTray| {
                tracing::info!("tray: Settings... requested");
                let _ = this.ui_tx.send(UiCmd::OpenSettings);
            }),
            ..Default::default()
        }.into(),
    ];

    ksni::menu::SubMenu {
        label: "LLM Refinement".into(),
        submenu,
        ..Default::default()
    }
    .into()
}
```

**Step 2:** Build + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
cd /home/desmond/Repos/voice-input-src
git add linux/src/tray.rs
git commit -m "feat(linux): tray LLM Refinement submenu (Enable + Settings...)"
```

---

## Task 5.7: GTK Settings dialog (form + Save)

**Files:** Modify `linux/src/settings_window.rs`.

GTK4 dialog with three Entry widgets (Base URL, API Key via PasswordEntry, Model) + status label + Test/Save buttons. Reads current Config via AppState.snapshot() on open; writes via AppState.update() on Save.

**Step 1:** Replace stub `linux/src/settings_window.rs` with the full implementation (paste from plan body — see implementer prompt for exact code).

Key API:
- `pub fn build_window(app: &Application, state: &AppState) -> ApplicationWindow`
- Reads from snapshot, populates 3 entries
- Save button calls `state.update(|cfg| { ...assign 3 fields... })?` and closes window on success
- Test button shows placeholder text (real wiring in Task 5.8)

**Step 2:** Build + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -10
cd /home/desmond/Repos/voice-input-src
git add linux/src/settings_window.rs
git commit -m "feat(linux): GTK Settings dialog with form fields + Save"
```

---

## Task 5.8: Settings dialog Test button

**Files:** Modify `linux/src/settings_window.rs`.

Replace the placeholder Test handler with a real `LlmRefiner::try_refine("Hello, this is a test.", force=true)` call using the field values (NOT the persisted Config — Test verifies edits before Save).

**Step 1:** Wire Test via `glib::MainContext::default().spawn_local(async move { ... })`. Build a one-shot refiner from a synthetic Config that mirrors the entered fields. Display result/error in the status label using inline pango markup (green for success, red for error).

**Step 2:** Build + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
cd /home/desmond/Repos/voice-input-src
git add linux/src/settings_window.rs
git commit -m "feat(linux): wire Settings Test button to try_refine + colored status"
```

---

## Task 5.9: Unify default mode (run_app)

**Files:** Modify `linux/src/main.rs`.

Delete `run_tray` / `run_tray_async`. Replace `run_listen` / `run_listen_async` with `run_app` / `run_backend_async`. Wire:
- AppState constructed once from cfg
- ksni tray spawned inside backend tokio runtime
- Listen loop reads `language_hint` from `state.snapshot()` per pipeline start
- Refiner rebuilt when `state.config_changed` fires
- Activated arm short-circuits when `!state.snapshot().enabled`
- GTK polling loop handles `UiCmd::OpenSettings` by building/presenting Settings dialog (with RefCell tracking the open instance)
- ctrlc handler notifies state.shutdown + sends UiCmd::Quit
- Default mode dispatch: `None | Some(Command::Listen) => run_app(cfg)`

This is the biggest task. Use multiple python heredocs to apply 6 edits to main.rs (full edit list in plan body).

**Step 1-6:** Apply edits via python heredoc.

**Step 7:** Build + test + commit.

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -15
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
cd /home/desmond/Repos/voice-input-src
git add linux/src/main.rs
git commit -m "feat(linux): unify default mode = tray + listen + overlay + Settings"
```

---

## Task 5.10: README + smoke test + final verification + push

**Files:** Modify `linux/README.md`; user-driven smoke test.

**Step 1:** Update README Status block + Run section to describe Phase 5 unified mode.

**Step 2:** cargo fmt + clippy.

**Step 3:** User smoke test:

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build --release
RUST_LOG=info ./target/release/voice-input
```

Acceptance checklist:
1. Tray icon appears with tooltip.
2. Menu shows Enabled / Language submenu / LLM Refinement submenu / Quit.
3. Toggle Enabled off -> hotkey ignored, persisted to config.toml.
4. Change Language -> next dictation uses new language.
5. LLM Refinement -> Enabled toggle, Settings... opens dialog.
6. Settings dialog: fields populate from config. Click Test (empty key -> red error, valid Ollama -> green OK). Click Save -> closes + persists.
7. Refiner reload: dictate again after Save -> log shows refiner rebuilt with new config.
8. Quit from tray -> clean exit within 1s.

**Step 4:** Push.

```bash
cd /home/desmond/Repos/voice-input-src
git push -u origin linux/phase-5-settings-tray-menus
```

---

## Self-Review Notes

**Spec coverage:**
- GTK Settings dialog -> Tasks 5.7 + 5.8
- Language submenu -> Task 5.5 + listen-loop reads snapshot per pipeline (Task 5.9)
- Enable toggle -> Tasks 5.1 + 5.4 + Task 5.9 listen-loop check
- LLM Refinement submenu (Enable + Settings...) -> Task 5.6 + 5.7
- Persistence via AppState.update() -> all menu/save callbacks
- Deferred to Phase 7: recording-state icon change
- Deferred to Phase 8: autostart .desktop

**Architectural decisions:**
- Single default mode (Task 5.9): matches macOS UX.
- AppState pattern (Task 5.3): parking_lot::Mutex<Config> + Notify events.
- UiCmd rename (Task 5.2): one channel serves overlay + Settings.
- Inline pango markup over CSS provider (Task 5.8): simpler.

**Known risks:**
- ksni SubMenu API verified in 0.3.
- gtk4 PasswordEntry needs show_peek_icon present in 0.10.
- glib::MainContext::spawn_local requires 'static future - clone all captures.
- RefCell<Option<ApplicationWindow>> for tracking open Settings window - check for recursive borrow in close-request handler.

**Implementer note:** This plan is a condensed summary. Each task in the implementer prompt will include the FULL code blocks (struct definitions, helper functions, edit scripts). Do not write code blindly from this summary; the implementer prompts include the verbatim text to paste.
