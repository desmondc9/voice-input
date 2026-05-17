use anyhow::Context;
use clap::Parser;
use gtk4::prelude::*;
use gtk4::Application;
use voice_input::{
    cli::{Cli, Command},
    config::Config,
    overlay::{self, OverlayWindow, UiCmd},
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
        None | Some(Command::Listen) => run_app(cfg),
        Some(Command::Transcribe) => run_transcribe(cfg),
    }
}

fn run_transcribe(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg
        .resolve_model_path()
        .context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting transcribe pipeline");

    let (_capture, pipeline) =
        voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone(), None)
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

    drop(_capture); // stop audio so VAD thread exits
    let _ = pipeline.join_remaining(); // drain any final segment we missed
    tracing::info!(
        "pipeline shutdown complete; transcribed {} segments",
        segment_count
    );
    Ok(())
}

fn run_app(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg
        .resolve_model_path()
        .context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting app (tray + listen + overlay)");

    voice_input::injector::verify_available()
        .context("ydotool must be installed and ydotoold running")?;

    // Shared state across tray + listen loop + Settings dialog.
    let state = voice_input::state::AppState::new(cfg);

    // Backend ↔ GTK channel.
    let (ui_tx, ui_rx) = overlay::channel();
    let ui_tx_for_signal = ui_tx.clone();

    // Spawn the backend thread (tokio: tray + portal + pipeline + LLM).
    let model_path_for_backend = model_path.clone();
    let state_for_backend = state.clone();
    let ui_tx_for_backend = ui_tx.clone();
    let backend = std::thread::Builder::new()
        .name("voice-input-backend".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime in backend thread");
            if let Err(e) = rt.block_on(run_backend_async(
                state_for_backend,
                model_path_for_backend,
                ui_tx_for_backend,
            )) {
                tracing::error!(error = %e, "backend exited with error");
            }
        })
        .context("spawning backend thread")?;

    // Run GTK on the OS main thread.
    let app = Application::builder()
        .application_id("com.yetone.VoiceInput")
        .build();

    let state_for_activate = state.clone();
    let ui_rx_cell = std::cell::RefCell::new(Some(ui_rx));
    app.connect_activate(move |app| {
        let window = OverlayWindow::new(app);
        window.hide();

        // Tracks the live Settings window (None when closed; Some when open).
        // Single-threaded GTK context, so RefCell is fine.
        let settings_window: std::rc::Rc<std::cell::RefCell<Option<gtk4::ApplicationWindow>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));

        let rx = ui_rx_cell
            .borrow_mut()
            .take()
            .expect("activate called once");
        let window_for_loop = window;
        let app_for_loop = app.clone();
        let state_for_loop = state_for_activate.clone();
        let settings_for_loop = settings_window.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            loop {
                match rx.try_recv() {
                    Ok(UiCmd::Show) => window_for_loop.show(),
                    Ok(UiCmd::Hide) => window_for_loop.hide(),
                    Ok(UiCmd::SetLevel(level)) => window_for_loop.set_level(level),
                    Ok(UiCmd::SetText(text)) => window_for_loop.set_text(&text),
                    Ok(UiCmd::OpenSettings) => {
                        let already_open = settings_for_loop.borrow().is_some();
                        if already_open {
                            tracing::info!("ui: re-presenting existing Settings window");
                            if let Some(win) = settings_for_loop.borrow().as_ref() {
                                win.present();
                            }
                        } else {
                            tracing::info!("ui: building Settings window");
                            let win = voice_input::settings_window::build_window(
                                &app_for_loop,
                                &state_for_loop,
                            );
                            let cell_for_close = settings_for_loop.clone();
                            win.connect_close_request(move |_| {
                                *cell_for_close.borrow_mut() = None;
                                gtk4::glib::Propagation::Proceed
                            });
                            win.present();
                            *settings_for_loop.borrow_mut() = Some(win);
                        }
                    }
                    Ok(UiCmd::Quit) => {
                        app_for_loop.quit();
                        return gtk4::glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        app_for_loop.quit();
                        return gtk4::glib::ControlFlow::Break;
                    }
                }
            }
            gtk4::glib::ControlFlow::Continue
        });
    });

    let shutdown_for_signal = state.shutdown.clone();
    let _ = ctrlc::set_handler(move || {
        shutdown_for_signal.notify_waiters();
        let _ = ui_tx_for_signal.send(UiCmd::Quit);
    });

    let exit_code = app.run_with_args::<&str>(&[]);
    state.shutdown.notify_waiters();
    let _ = backend.join();

    if exit_code.get() != 0 {
        anyhow::bail!("gtk application exited with code {}", exit_code.get());
    }
    Ok(())
}

async fn run_backend_async(
    state: voice_input::state::AppState,
    model_path: std::path::PathBuf,
    overlay_tx: overlay::UiSender,
) -> anyhow::Result<()> {
    use futures_util::stream::StreamExt;
    use ksni::TrayMethods;
    use voice_input::hotkey::HotkeyHandle;
    use voice_input::speech;
    use voice_input::tray::VoiceInputTray;

    // Spawn the tray inside this tokio runtime.
    let tray = VoiceInputTray::new(state.clone(), overlay_tx.clone());
    let _tray_handle = tray.spawn().await.context("spawning tray")?;
    tracing::info!("tray spawned");

    let hotkey = HotkeyHandle::create()
        .await
        .context("creating portal global-shortcuts session")?;
    let mut activated = hotkey.activated().await.context("activated stream")?;
    let mut deactivated = hotkey.deactivated().await.context("deactivated stream")?;

    tracing::info!(
        "app running — hold the bound shortcut to dictate (when Enabled); Ctrl+C to exit"
    );

    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let mut refiner = voice_input::refiner::LlmRefiner::from_config(&state.snapshot());
    tracing::info!(active = refiner.is_active(), "llm refiner initialized");

    // RMS level fan-out: vad-resample thread → blocking task → overlay channel.
    // We use a bounded crossbeam channel sized for ~1s of audio at 100Hz.
    let (level_tx, level_rx) = crossbeam_channel::bounded::<f32>(128);
    let overlay_tx_for_levels = overlay_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(level) = level_rx.recv() {
            // Stop forwarding when the overlay channel is closed.
            if overlay_tx_for_levels.send(UiCmd::SetLevel(level)).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            biased;
            _ = state.shutdown.notified() => {
                tracing::info!("shutdown signaled; exiting app");
                break;
            }
            _ = state.config_changed.notified() => {
                refiner = voice_input::refiner::LlmRefiner::from_config(&state.snapshot());
                tracing::info!(active = refiner.is_active(), "llm refiner rebuilt from updated config");
            }
            Some(_activated) = activated.next() => {
                if current_pipeline.is_some() {
                    continue;
                }
                let snap = state.snapshot();
                if !snap.enabled {
                    tracing::info!("shortcut pressed but Enabled=false; ignoring");
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, snap.language_hint.clone(), Some(level_tx.clone())) {
                    Ok((capture, p)) => {
                        let _ = overlay_tx.send(UiCmd::Show);
                        current_capture = Some(capture);
                        current_pipeline = Some(p);
                    }
                    Err(e) => tracing::error!(error = %e, "failed to start pipeline"),
                }
            }
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    drop(current_capture.take());
                    let segments = tokio::task::spawn_blocking(move || pipeline.join_remaining())
                        .await
                        .context("draining pipeline")?;
                    let raw_joined = segments.join(" ").trim().to_string();
                    if raw_joined.is_empty() {
                        tracing::info!("no segments transcribed; skipping paste");
                    } else {
                        // Refine before paste. The refiner short-circuits when
                        // disabled/unconfigured; on errors it logs and returns the
                        // raw text — paste must not fail because the LLM is down.
                        let _ = overlay_tx.send(UiCmd::SetText("Refining…".into()));
                        let to_paste = refiner.refine(&raw_joined, false).await;
                        tracing::info!(
                            segments = segments.len(),
                            raw_bytes = raw_joined.len(),
                            final_bytes = to_paste.len(),
                            "pasting"
                        );
                        let injected = tokio::task::spawn_blocking({
                            let to_paste = to_paste.clone();
                            move || voice_input::injector::inject_text(&to_paste)
                        })
                        .await
                        .context("ydotool paste task")?;
                        if let Err(e) = injected {
                            tracing::error!(error = %e, "paste failed");
                        }
                    }
                    let _ = overlay_tx.send(UiCmd::Hide);
                }
            }
            else => {
                tracing::warn!("portal streams closed; exiting");
                break;
            }
        }
    }

    // Tell the GTK main loop to quit so the process exits cleanly.
    let _ = overlay_tx.send(UiCmd::Hide);
    let _ = overlay_tx.send(UiCmd::Quit);
    Ok(())
}
