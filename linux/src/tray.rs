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
            language_menu(&snap.language_hint),
            llm_menu(snap.llm_enabled),
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


/// Whisper language codes shown in the tray. Empty string = auto-detect.
/// Native-script labels match macOS AppDelegate.swift:190-197 adapted
/// from BCP-47 to ISO 639-1 (whisper.cpp's format).
const LANGUAGES: &[(&str, &str)] = &[
    ("Auto-detect", ""),
    ("English", "en"),
    ("中文", "zh"),
    ("日本語", "ja"),
    ("한국어", "ko"),
    ("Español", "es"),
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
                    if let Err(e) =
                        this.state.update(|cfg| cfg.language_hint = code_clone.clone())
                    {
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
        }
        .into(),
        MenuItem::Separator,
        StandardItem {
            label: "Settings…".into(),
            activate: Box::new(|this: &mut VoiceInputTray| {
                tracing::info!("tray: Settings… requested");
                let _ = this.ui_tx.send(UiCmd::OpenSettings);
            }),
            ..Default::default()
        }
        .into(),
    ];

    ksni::menu::SubMenu {
        label: "LLM Refinement".into(),
        submenu,
        ..Default::default()
    }
    .into()
}
