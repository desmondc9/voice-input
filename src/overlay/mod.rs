//! GTK4 + layer-shell overlay capsule shown during `listen` mode.
//!
//! Lives entirely on the OS main thread. The backend thread (where the
//! ashpd portal + speech pipeline + tray run) sends `UiCmd` messages via a
//! `std::sync::mpsc` channel; the main thread receives and applies them
//! to the `OverlayWindow` / `WaveformView` widgets or opens the Settings
//! dialog.

pub mod waveform;
pub mod window;

pub use window::OverlayWindow;

use std::sync::mpsc;

/// Commands the backend thread sends to the GTK main thread.
#[derive(Debug, Clone)]
pub enum UiCmd {
    /// Hotkey pressed — make the capsule visible.
    Show,
    /// Updated audio level in [0, 1]. Drives waveform animation.
    SetLevel(f32),
    /// Replace the text label content. Used for state transitions
    /// ("Listening…", "Refining…", future partial transcripts).
    SetText(String),
    /// Hotkey released and paste completed — hide the capsule.
    Hide,
    /// Backend is shutting down (Ctrl+C). The GTK loop should quit too.
    Quit,
    /// Tray "Settings…" clicked — open (or re-focus) the Settings dialog.
    OpenSettings,
}

/// Backend → main channel.
///
/// We use `std::sync::mpsc` here (NOT `glib::MainContext::channel`) so the
/// `UiSender` is `Send` and can be cloned/moved into the backend thread
/// without GTK headers being in scope. The GTK loop drains via
/// `glib::timeout_add_local` polling the receiver — this trades CPU for
/// simplicity. A future polish task can swap to `glib` channels for true
/// event-driven dispatch.
pub type UiSender = mpsc::Sender<UiCmd>;
pub type UiReceiver = mpsc::Receiver<UiCmd>;

pub fn channel() -> (UiSender, UiReceiver) {
    mpsc::channel()
}
