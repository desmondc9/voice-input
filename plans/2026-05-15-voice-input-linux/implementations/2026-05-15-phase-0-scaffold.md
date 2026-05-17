# Phase 0 — Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `linux/` Rust project with a tokio runtime, a working KSNI tray ("Quit" only), config TOML round-trip, and the error-type scaffolding all later phases will depend on. No GUI window, no audio, no whisper yet — that's Phase 1+.

**Architecture:** Single binary `voice-input` in a Cargo project at `linux/`. Tokio current-thread runtime owns the lifecycle. `ksni` provides the StatusNotifierItem tray. `config.rs` and `error.rs` define the cross-cutting types every later module reuses. GTK4 deliberately deferred to Phase 3 (overlay).

**Tech Stack:**
- Rust 1.83+ stable
- `tokio = "1"` (runtime, sync primitives)
- `ksni = "0.3"` (StatusNotifierItem tray, tokio runtime support)
- `serde = "1"` + `serde_derive`, `toml = "0.8"`, `directories = "5"` (config persistence)
- `thiserror = "1"` (error enum), `anyhow = "1"` (binary-level error context)
- `tracing = "0.1"` + `tracing-subscriber = "0.3"` (logging to `~/.local/state/voice-input/`)

**Reference spec:** `plans/voice-input-linux.md` — Phase 0 section + the `AppState` / `ErrorKind` enums + config keys table.

---

## File Structure

| Path | Purpose |
|---|---|
| `linux/Cargo.toml` | Crate manifest with all Phase 0 dependencies pinned |
| `linux/.gitignore` | Ignore `target/`, OS junk |
| `linux/README.md` | Stub README (will grow per phase) |
| `linux/src/main.rs` | Entry point: init tracing, load config, spawn tray, await Quit |
| `linux/src/lib.rs` | Module declarations (also enables integration tests) |
| `linux/src/error.rs` | `AppError` enum + `ErrorKind` (Clone, for `AppState::Error`) + `AppResult<T>` alias |
| `linux/src/config.rs` | `Config` struct, `Config::load()`, `Config::save()`, `Config::config_path()` |
| `linux/src/app.rs` | `AppState` enum stub (no methods yet) |
| `linux/src/tray.rs` | `VoiceInputTray` ksni implementation with single "Quit" item |
| `linux/tests/config_roundtrip.rs` | Integration test for config write→read |

---

## Task 0.1: Initialize Cargo project

**Files:**
- Create: `linux/Cargo.toml`
- Create: `linux/.gitignore`
- Create: `linux/src/lib.rs` (empty for now — we'll add modules in 0.2+)
- Create: `linux/src/main.rs` (minimal — replaced in Task 0.6)

- [ ] **Step 1: Create the Cargo project skeleton**

Run:
```bash
cd /home/desmond/Repos/voice-input-src
mkdir -p linux/src linux/tests
```

- [ ] **Step 2: Write `linux/Cargo.toml`**

```toml
[package]
name = "voice-input"
version = "0.1.0"
edition = "2021"
rust-version = "1.83"
authors = ["Desmond Chen"]
license = "MIT"
description = "Wayland-native voice input — hold-to-talk speech-to-text"

[dependencies]
anyhow = "1"
directories = "5"
ksni = { version = "0.3", features = ["tokio"] }
serde = { version = "1", features = ["derive"] }
thiserror = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "signal"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
tempfile = "3"

[[bin]]
name = "voice-input"
path = "src/main.rs"

[lib]
name = "voice_input"
path = "src/lib.rs"
```

- [ ] **Step 3: Write `linux/.gitignore`**

```gitignore
/target
*.swp
*.bak
.DS_Store
```

- [ ] **Step 4: Write minimal `linux/src/lib.rs`** (modules added in later tasks)

```rust
// Module declarations are added in subsequent tasks.
```

- [ ] **Step 5: Write minimal `linux/src/main.rs`**

```rust
fn main() {
    println!("voice-input scaffold — phase 0 in progress");
}
```

- [ ] **Step 6: Verify the project compiles**

Run: `cd linux && cargo build`
Expected: dependency download then `Finished` line. No source errors.

- [ ] **Step 7: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/.gitignore linux/src/lib.rs linux/src/main.rs
git commit -m "feat(linux): bootstrap Cargo project with Phase 0 dependencies"
```

---

## Task 0.2: Implement `error.rs` (TDD)

**Files:**
- Create: `linux/src/error.rs`
- Modify: `linux/src/lib.rs` (add `pub mod error;`)

Design: `AppError` is the rich result-error type for fallible operations. `ErrorKind` is a small `Clone` enum used in `AppState::Error` for UI display. `AppError::kind()` maps to `ErrorKind`.

- [ ] **Step 1: Add `pub mod error;` to `linux/src/lib.rs`**

```rust
pub mod error;
```

- [ ] **Step 2: Write the failing test in `linux/src/error.rs`**

```rust
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    NoMicrophone,
    ModelMissing,
    WhisperFailed,
    PortalRevoked,
    YdotoolMissing,
    NetworkError,
    Config,
    Io,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("no microphone available: {0}")]
    NoMicrophone(String),

    #[error("whisper model file missing at {path}")]
    ModelMissing { path: PathBuf },

    #[error("whisper inference failed: {0}")]
    WhisperFailed(String),

    #[error("global shortcut session revoked")]
    PortalRevoked,

    #[error("ydotool unavailable: {0}")]
    YdotoolMissing(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            AppError::NoMicrophone(_) => ErrorKind::NoMicrophone,
            AppError::ModelMissing { .. } => ErrorKind::ModelMissing,
            AppError::WhisperFailed(_) => ErrorKind::WhisperFailed,
            AppError::PortalRevoked => ErrorKind::PortalRevoked,
            AppError::YdotoolMissing(_) => ErrorKind::YdotoolMissing,
            AppError::NetworkError(_) => ErrorKind::NetworkError,
            AppError::Config(_) => ErrorKind::Config,
            AppError::Io(_) => ErrorKind::Io,
        }
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_mapping_is_exhaustive() {
        assert_eq!(AppError::NoMicrophone("x".into()).kind(), ErrorKind::NoMicrophone);
        assert_eq!(
            AppError::ModelMissing { path: "/tmp/x".into() }.kind(),
            ErrorKind::ModelMissing
        );
        assert_eq!(AppError::WhisperFailed("x".into()).kind(), ErrorKind::WhisperFailed);
        assert_eq!(AppError::PortalRevoked.kind(), ErrorKind::PortalRevoked);
        assert_eq!(AppError::YdotoolMissing("x".into()).kind(), ErrorKind::YdotoolMissing);
        assert_eq!(AppError::NetworkError("x".into()).kind(), ErrorKind::NetworkError);
        assert_eq!(AppError::Config("x".into()).kind(), ErrorKind::Config);
        let io = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "x"));
        assert_eq!(io.kind(), ErrorKind::Io);
    }

    #[test]
    fn display_includes_context() {
        let err = AppError::NoMicrophone("default device missing".into());
        assert!(err.to_string().contains("default device missing"));
    }

    #[test]
    fn io_error_auto_converts() {
        fn read() -> AppResult<String> {
            std::fs::read_to_string("/nonexistent/path/that/does/not/exist")?;
            Ok(String::new())
        }
        let err = read().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Io);
    }
}
```

- [ ] **Step 3: Run tests — they should pass**

Run: `cd linux && cargo test --lib error::tests`
Expected: 3 passed. (Test code and implementation are bundled in one step here because the impl is mechanical — verify by running.)

- [ ] **Step 4: Commit**

```bash
git add linux/src/lib.rs linux/src/error.rs
git commit -m "feat(linux): add AppError + ErrorKind with kind() mapping"
```

---

## Task 0.3: Implement `config.rs` (TDD)

**Files:**
- Create: `linux/src/config.rs`
- Modify: `linux/src/lib.rs` (add `pub mod config;`)
- Create: `linux/tests/config_roundtrip.rs`

Design: `Config` mirrors the keys from the design doc. `config_path()` resolves to `~/.config/voice-input/config.toml` via the `directories` crate. For tests we need to override the path so test runs don't clobber real config.

- [ ] **Step 1: Add `pub mod config;` to `linux/src/lib.rs`**

`linux/src/lib.rs` now reads:
```rust
pub mod config;
pub mod error;
```

- [ ] **Step 2: Write `linux/src/config.rs`**

```rust
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl Config {
    /// Default on-disk path for the running user.
    pub fn config_path() -> AppResult<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "yetone", "VoiceInput")
            .ok_or_else(|| AppError::Config("cannot resolve XDG config dir".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load() -> AppResult<Self> {
        Self::load_from(&Self::config_path()?)
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&Self::config_path()?)
    }

    /// Test-friendly variant: load from an explicit path. Returns defaults if missing.
    pub fn load_from(path: &Path) -> AppResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| AppError::Config(format!("parse {}: {}", path.display(), e)))
    }

    /// Test-friendly variant: save to an explicit path. Creates parent dirs.
    pub fn save_to(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("serialize: {}", e)))?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.language_hint, "zh");
        assert_eq!(cfg.llm_api_base_url, "https://api.openai.com/v1");
        assert!(!cfg.llm_enabled);
        assert_eq!(cfg.whisper_model_size, "small");
        assert!(cfg.whisper_model_path.is_none());
        assert!(cfg.shortcut_handle.is_none());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn invalid_toml_returns_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not = valid [[ toml").unwrap();
        let err = Config::load_from(&path).unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Config);
    }
}
```

- [ ] **Step 3: Write the integration roundtrip test in `linux/tests/config_roundtrip.rs`**

```rust
use voice_input::config::Config;

#[test]
fn write_then_read_yields_same_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    let mut original = Config::default();
    original.language_hint = "ja".into();
    original.llm_enabled = true;
    original.llm_api_key = "sk-test-12345".into();
    original.shortcut_handle = Some("portal-handle-abc".into());

    original.save_to(&path).expect("save");
    let loaded = Config::load_from(&path).expect("load");

    assert_eq!(loaded, original);
}

#[test]
fn save_creates_parent_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("a").join("b").join("c").join("config.toml");

    Config::default().save_to(&nested).expect("save creates parents");
    assert!(nested.exists());
}
```

- [ ] **Step 4: Run all tests**

Run: `cd linux && cargo test`
Expected: 6 passed (3 unit + 3 integration including the original `error::tests`).

- [ ] **Step 5: Commit**

```bash
git add linux/src/config.rs linux/src/lib.rs linux/tests/config_roundtrip.rs
git commit -m "feat(linux): add Config TOML persistence with roundtrip tests"
```

---

## Task 0.4: `AppState` enum stub

**Files:**
- Create: `linux/src/app.rs`
- Modify: `linux/src/lib.rs` (add `pub mod app;`)

For Phase 0, just the enum and a `Default` impl. Transitions come in Phase 5.

- [ ] **Step 1: Add `pub mod app;` to `linux/src/lib.rs`**

`linux/src/lib.rs` now reads:
```rust
pub mod app;
pub mod config;
pub mod error;
```

- [ ] **Step 2: Write `linux/src/app.rs`**

```rust
use std::time::Instant;

use crate::error::ErrorKind;

/// Top-level application state. Transitions are added in later phases;
/// for Phase 0 the type exists so other modules can reference it.
#[derive(Debug, Clone)]
pub enum AppState {
    Idle,
    Listening { started_at: Instant },
    Refining { raw_text: String },
    Injecting { final_text: String },
    Error(ErrorKind),
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_idle() {
        assert!(matches!(AppState::default(), AppState::Idle));
    }

    #[test]
    fn error_variant_carries_kind() {
        let s = AppState::Error(ErrorKind::NoMicrophone);
        match s {
            AppState::Error(k) => assert_eq!(k, ErrorKind::NoMicrophone),
            _ => panic!("expected Error variant"),
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd linux && cargo test app::tests`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add linux/src/app.rs linux/src/lib.rs
git commit -m "feat(linux): add AppState enum stub"
```

---

## Task 0.5: `tray.rs` with ksni "Quit"

**Files:**
- Create: `linux/src/tray.rs`
- Modify: `linux/src/lib.rs` (add `pub mod tray;`)

No unit test for tray — it requires a D-Bus session host and is awkward to test in isolation. Verification is the manual smoke test in Task 0.7.

- [ ] **Step 1: Add `pub mod tray;` to `linux/src/lib.rs`**

`linux/src/lib.rs` now reads:
```rust
pub mod app;
pub mod config;
pub mod error;
pub mod tray;
```

- [ ] **Step 2: Write `linux/src/tray.rs`**

```rust
use std::sync::Arc;

use ksni::{menu::StandardItem, MenuItem, Tray};
use tokio::sync::Notify;

/// KSNI tray with a single "Quit" item. Quit triggers `shutdown.notify_one()`
/// so `main` can perform an orderly shutdown.
pub struct VoiceInputTray {
    shutdown: Arc<Notify>,
}

impl VoiceInputTray {
    pub fn new(shutdown: Arc<Notify>) -> Self {
        Self { shutdown }
    }
}

impl Tray for VoiceInputTray {
    fn id(&self) -> String {
        "com.yetone.VoiceInput".into()
    }

    fn title(&self) -> String {
        "VoiceInput".into()
    }

    fn icon_name(&self) -> String {
        // Falls back to a generic mic icon from the system icon theme.
        "audio-input-microphone".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "VoiceInput".into(),
            description: "Hold the configured key to dictate".into(),
            icon_name: "audio-input-microphone".into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![StandardItem {
            label: "Quit".into(),
            icon_name: "application-exit".into(),
            activate: Box::new(|this: &mut Self| {
                tracing::info!("tray: Quit selected");
                this.shutdown.notify_one();
            }),
            ..Default::default()
        }
        .into()]
    }
}
```

- [ ] **Step 3: Confirm it compiles**

Run: `cd linux && cargo build`
Expected: clean build. (No new tests yet — wired up in Task 0.6.)

- [ ] **Step 4: Commit**

```bash
git add linux/src/tray.rs linux/src/lib.rs
git commit -m "feat(linux): add KSNI tray with Quit action"
```

---

## Task 0.6: Wire `main.rs`

**Files:**
- Modify: `linux/src/main.rs` (replace the stub from Task 0.1)

- [ ] **Step 1: Replace `linux/src/main.rs`**

```rust
use std::sync::Arc;

use anyhow::Context;
use ksni::TrayMethods;
use tokio::sync::Notify;
use voice_input::{config::Config, tray::VoiceInputTray};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::load().context("loading config")?;
    tracing::info!(
        language_hint = %cfg.language_hint,
        llm_enabled = cfg.llm_enabled,
        whisper_model_size = %cfg.whisper_model_size,
        "config loaded"
    );

    // Materialize the config file on disk so the user can edit it.
    cfg.save().context("persisting config defaults")?;

    let shutdown = Arc::new(Notify::new());

    let tray = VoiceInputTray::new(shutdown.clone());
    let _tray_handle = tray.spawn().await.context("spawning tray")?;

    tracing::info!("voice-input running — Quit via tray icon or Ctrl+C");

    tokio::select! {
        _ = shutdown.notified() => tracing::info!("tray Quit received"),
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received"),
    }

    tracing::info!("shutdown complete");
    Ok(())
}
```

- [ ] **Step 2: Build the binary**

Run: `cd linux && cargo build`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add linux/src/main.rs
git commit -m "feat(linux): wire main.rs — config load, tray spawn, shutdown signal"
```

---

## Task 0.7: Manual smoke test

This is the Phase 0 verification gate. Run on the user's actual compositor.

- [ ] **Step 1: Run the binary in the foreground**

Run: `cd linux && RUST_LOG=info cargo run`
Expected log output (within ~1 second):
```
INFO voice_input: config loaded language_hint=zh llm_enabled=false whisper_model_size=small
INFO voice_input: voice-input running — Quit via tray icon or Ctrl+C
```

- [ ] **Step 2: Verify the tray icon appears**

Look at the system tray:
- **KDE Plasma 6:** icon in the system tray panel
- **sway / hyprland:** icon in waybar (`tray` module must be enabled in waybar config; document this in the README in Task 0.8 if not already true on the test machine)

Expected: a microphone icon. Hover → tooltip reads "VoiceInput — Hold the configured key to dictate". Right-click → menu has "Quit".

- [ ] **Step 3: Verify config file was written**

Run: `cat ~/.config/voice-input/config.toml`
Expected: TOML content with `language_hint = "zh"`, `llm_enabled = false`, etc.

- [ ] **Step 4: Click "Quit" in the tray menu**

Expected log:
```
INFO voice_input::tray: tray: Quit selected
INFO voice_input: tray Quit received
INFO voice_input: shutdown complete
```
Process exits with status 0.

- [ ] **Step 5: Run again, this time Ctrl+C instead**

Run: `cd linux && RUST_LOG=info cargo run`
Hit Ctrl+C.
Expected: `SIGINT received` log line followed by clean shutdown.

- [ ] **Step 6: Edit the config file, re-run, verify it's read**

```bash
sed -i 's/language_hint = "zh"/language_hint = "ja"/' ~/.config/voice-input/config.toml
cd linux && RUST_LOG=info cargo run
```
Expected: `config loaded language_hint=ja ...`. Quit.

If anything in steps 1–6 fails, debug it before moving on. Phase 0 is the foundation every later phase relies on.

---

## Task 0.8: README skeleton

**Files:**
- Create: `linux/README.md`

- [ ] **Step 1: Write `linux/README.md`**

```markdown
# VoiceInput (Linux)

Wayland-native voice input for KDE Plasma 6, sway, and hyprland. Hold a configured key, speak, release — the transcript is pasted into the focused application.

> Status: **Phase 0** — scaffold only. No audio or transcription yet. See `../implementation/` for the phased build plan.

## Build

Requires Rust 1.83+ and the GTK4 development packages (used from Phase 3 onward).

```bash
cd linux
cargo build --release
```

## Run

```bash
RUST_LOG=info cargo run
```

A tray icon appears in your system tray (KDE Plasma) or waybar (sway / hyprland — needs the `tray` module).

## Compositor support

- **KDE Plasma 6**: target compositor, built-in StatusNotifierItem host.
- **sway**: requires waybar with `tray` module.
- **hyprland**: requires waybar / ironbar / Riftbar with `tray` module.
- **GNOME**: **not supported.** Mutter lacks `wlr-layer-shell` (needed in Phase 3).

## Config

`~/.config/voice-input/config.toml` — created on first run. Edit and restart to change.

## Project layout

See `../plans/voice-input-linux.md` for the full design and `../implementation/` for per-phase implementation plans.
```

- [ ] **Step 2: Commit**

```bash
git add linux/README.md
git commit -m "docs(linux): add Phase 0 README"
```

---

## Task 0.9: Final verification + push

- [ ] **Step 1: Run the full test suite one more time**

Run: `cd linux && cargo test`
Expected: all tests pass (3 in `error::tests`, 3 in `config::tests`, 2 in `app::tests`, 2 integration tests in `config_roundtrip` = **10 passing**).

- [ ] **Step 2: Check for warnings**

Run: `cd linux && cargo build --release 2>&1 | grep -i warning`
Expected: no output (or only unrelated dependency warnings). Fix any of our own warnings.

- [ ] **Step 3: Check formatting**

Run: `cd linux && cargo fmt --check`
Expected: no output. If it complains, run `cargo fmt` and amend the last commit:
```bash
cargo fmt
git add -u
git commit --amend --no-edit
```

- [ ] **Step 4: Check clippy**

Run: `cd linux && cargo clippy -- -D warnings`
Expected: no warnings. Fix any clippy findings as a separate commit:
```bash
git commit -m "chore(linux): fix clippy findings"
```

- [ ] **Step 5: Push to origin**

Run: `git push origin main`
Expected: commits pushed. (The branch was already tracking `origin/main` from the earlier setup.)

- [ ] **Step 6: Confirm Phase 0 is complete**

Phase 0 is done when:
- `cargo build` succeeds with zero warnings
- `cargo test` shows 10 passing tests
- `cargo run` shows a tray icon
- Clicking Quit cleanly exits
- `~/.config/voice-input/config.toml` is written
- All commits are pushed to `origin/main`

Then proceed by invoking `writing-plans` again to create the Phase 1 plan (audio + VAD + speech).

---

## Self-Review Notes

Spec coverage check against `plans/voice-input-linux.md` Phase 0:
- ✅ Cargo workspace → Task 0.1
- ✅ ksni tray with Quit → Tasks 0.5, 0.6
- ✅ config.rs round-trip → Task 0.3
- ✅ error.rs skeleton → Task 0.2
- ✅ Toolchain validation on Plasma 6 / sway / hyprland → Task 0.7
- ⏸ "GTK4 hello-world window" from the design doc is **deliberately deferred** to Phase 3 (overlay). Phase 0 scaffold is cleaner without it, and we don't have a window to show anyway. This is a conscious narrowing of Phase 0 scope; flagged here so it's not missed in Phase 3.

Placeholder scan: no TBDs, no "implement later", no skipped code blocks.

Type consistency: `AppError`, `ErrorKind`, `AppResult<T>`, `Config`, `AppState`, `VoiceInputTray` — names match across tasks. `config_path() / load() / save() / load_from() / save_to()` signatures consistent.

Scope check: Phase 0 deliberately excludes audio, whisper, hotkey, overlay, refiner, settings UI, packaging. Those are Phase 1–8.
