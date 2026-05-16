use ksni::{menu::CheckmarkItem, menu::StandardItem, MenuItem, Tray};

use crate::overlay::{UiCmd, UiSender};
use crate::state::AppState;

/// KSNI tray for VoiceInput — main user-facing UI besides the overlay.
///
/// Menu structure (matches macOS AppDelegate.swift:175-234):
/// - Enabled (checkbox)
/// - Language ▶  (submenu — Task 5.5)
/// - LLM Refinement ▶  (submenu — Task 5.6)
/// - ---
/// - Quit
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
    fn id(&self) -> String {
        "com.yetone.VoiceInput".into()
    }

    fn title(&self) -> String {
        "VoiceInput".into()
    }

    fn icon_name(&self) -> String {
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
        let snap = self.state.snapshot();

        vec![
            CheckmarkItem {
                label: "Enabled".into(),
                checked: snap.enabled,
                activate: Box::new(|this: &mut Self| {
                    let new_value = !this.state.snapshot().enabled;
                    if let Err(e) = this.state.update(|cfg| cfg.enabled = new_value) {
                        tracing::error!(error = %e, "tray: failed to persist Enabled toggle");
                    } else {
                        tracing::info!(enabled = new_value, "tray: Enabled toggled");
                    }
                }),
                ..Default::default()
            }
            .into(),
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
            }
            .into(),
        ]
    }
}
