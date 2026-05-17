//! GTK4 Settings dialog for LLM refiner configuration.
//!
//! Mirrors macOS `SettingsWindow.swift`: three Entry widgets (Base URL,
//! API Key, Model) + a status label + Test/Save buttons. Reads from
//! `AppState` on open; writes back via `AppState::update()` on Save.
//!
//! The Test button is wired in Task 5.8 — currently it just sets a
//! placeholder status string.

use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Entry, Grid, Label, Orientation,
    PasswordEntry,
};

use crate::state::AppState;

pub fn build_window(app: &Application, state: &AppState) -> ApplicationWindow {
    let snap = state.snapshot();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("LLM Refinement Settings")
        .default_width(520)
        .default_height(260)
        .resizable(false)
        .build();

    let content = GtkBox::new(Orientation::Vertical, 16);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);

    let grid = Grid::new();
    grid.set_row_spacing(12);
    grid.set_column_spacing(10);

    // Row 0: API Base URL
    let url_label = Label::new(Some("API Base URL:"));
    url_label.set_halign(Align::End);
    let url_entry = Entry::builder()
        .text(snap.llm_api_base_url.as_str())
        .placeholder_text("https://api.openai.com/v1")
        .hexpand(true)
        .build();
    grid.attach(&url_label, 0, 0, 1, 1);
    grid.attach(&url_entry, 1, 0, 1, 1);

    // Row 1: API Key (masked with peek icon)
    let key_label = Label::new(Some("API Key:"));
    key_label.set_halign(Align::End);
    let key_entry = PasswordEntry::builder()
        .text(snap.llm_api_key.as_str())
        .placeholder_text("sk-...")
        .show_peek_icon(true)
        .hexpand(true)
        .build();
    grid.attach(&key_label, 0, 1, 1, 1);
    grid.attach(&key_entry, 1, 1, 1, 1);

    // Row 2: Model
    let model_label = Label::new(Some("Model:"));
    model_label.set_halign(Align::End);
    let model_entry = Entry::builder()
        .text(snap.llm_model.as_str())
        .placeholder_text("gpt-4o-mini")
        .hexpand(true)
        .build();
    grid.attach(&model_label, 0, 2, 1, 1);
    grid.attach(&model_entry, 1, 2, 1, 1);

    content.append(&grid);

    // Status label (populated by Test in Task 5.8)
    let status_label = Label::new(None);
    status_label.set_halign(Align::Start);
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    content.append(&status_label);

    // Button row — right-aligned
    let button_row = GtkBox::new(Orientation::Horizontal, 8);
    button_row.set_halign(Align::End);
    let test_button = Button::with_label("Test");
    let save_button = Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    button_row.append(&test_button);
    button_row.append(&save_button);
    content.append(&button_row);

    window.set_child(Some(&content));

    // Save: persist all three fields via AppState::update, then close.
    {
        let state = state.clone();
        let window_for_save = window.clone();
        let url_entry = url_entry.clone();
        let key_entry = key_entry.clone();
        let model_entry = model_entry.clone();
        save_button.connect_clicked(move |_| {
            let url = url_entry.text().to_string();
            let key = key_entry.text().to_string();
            let model = model_entry.text().to_string();
            let result = state.update(|cfg| {
                cfg.llm_api_base_url = url.clone();
                cfg.llm_api_key = key.clone();
                cfg.llm_model = model.clone();
            });
            match result {
                Ok(()) => {
                    tracing::info!("settings: saved");
                    window_for_save.close();
                }
                Err(e) => {
                    tracing::error!(error = %e, "settings: save failed");
                }
            }
        });
    }

    // Test: builds a one-shot LlmRefiner from the CURRENT field values
    // (NOT the persisted Config) and calls try_refine(force=true) so the
    // refiner's enabled/configured guard is bypassed.
    //
    // reqwest's async runtime is tokio — the GLib MainContext can't drive
    // it (the original spawn_local implementation hung forever). We run
    // the HTTP call on a dedicated std thread with its own single-threaded
    // tokio runtime, ship the result back over a crossbeam channel, and
    // let GTK poll for completion via glib::timeout_add_local.
    {
        let status_label = status_label.clone();
        let url_entry = url_entry.clone();
        let key_entry = key_entry.clone();
        let model_entry = model_entry.clone();
        let snapshot = state.snapshot();
        test_button.connect_clicked(move |_| {
            let url = url_entry.text().to_string();
            let key = key_entry.text().to_string();
            let model = model_entry.text().to_string();

            if key.trim().is_empty() {
                status_label.set_text("API key is empty");
                apply_status_color(&status_label, StatusKind::Error);
                return;
            }
            status_label.set_text("Testing…");
            apply_status_color(&status_label, StatusKind::Muted);

            let mut probe_cfg = snapshot.clone();
            probe_cfg.llm_api_base_url = url;
            probe_cfg.llm_api_key = key;
            probe_cfg.llm_model = model;
            let timeout_secs = probe_cfg.llm_timeout_secs;
            let refiner = crate::refiner::LlmRefiner::from_config(&probe_cfg);

            let (tx, rx) = crossbeam_channel::bounded::<crate::error::AppResult<String>>(1);

            std::thread::Builder::new()
                .name("voice-input-test-refine".into())
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("build tokio runtime for Test button");
                    // Hard timeout = client timeout + small buffer; protects
                    // against any path that bypasses the reqwest timeout.
                    let result = rt.block_on(async {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(timeout_secs + 5),
                            refiner.try_refine("Hello, this is a test.", true),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => Err(crate::error::AppError::NetworkError(format!(
                                "test request exceeded {}s hard timeout",
                                timeout_secs + 5
                            ))),
                        }
                    });
                    let _ = tx.send(result);
                })
                .expect("spawn test thread");

            let status_label = status_label.clone();
            gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                match rx.try_recv() {
                    Ok(Ok(text)) => {
                        let truncated = if text.chars().count() > 200 {
                            let prefix: String = text.chars().take(200).collect();
                            format!("{}…", prefix)
                        } else {
                            text
                        };
                        status_label.set_text(&format!("OK: {}", truncated));
                        apply_status_color(&status_label, StatusKind::Success);
                        gtk4::glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        status_label.set_text(&e.to_string());
                        apply_status_color(&status_label, StatusKind::Error);
                        gtk4::glib::ControlFlow::Break
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        gtk4::glib::ControlFlow::Continue
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        status_label.set_text("Test thread exited unexpectedly");
                        apply_status_color(&status_label, StatusKind::Error);
                        gtk4::glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    window
}

#[derive(Clone, Copy)]
enum StatusKind {
    Success,
    Error,
    Muted,
}

/// Re-render the label's current text with a pango foreground span so the
/// status conveys success/error/muted color. We use inline markup rather
/// than wiring a CSS provider -- Label::set_markup is built-in.
fn apply_status_color(label: &gtk4::Label, kind: StatusKind) {
    let text = label.text();
    let escaped = gtk4::glib::markup_escape_text(&text);
    let color = match kind {
        StatusKind::Success => "#2ec27e",
        StatusKind::Error => "#e01b24",
        StatusKind::Muted => "#888888",
    };
    label.set_markup(&format!(
        "<span foreground=\"{}\">{}</span>",
        color, escaped
    ));
}
