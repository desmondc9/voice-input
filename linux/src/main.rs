use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use gtk4::prelude::*;
use gtk4::Application;
use ksni::TrayMethods;
use tokio::sync::Notify;
use voice_input::{
    cli::{Cli, Command},
    config::Config,
    overlay::{self, UiCmd, OverlayWindow},
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
        Some(Command::Listen) => run_listen(cfg),
    }
}

fn run_tray(_cfg: Config) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(async { run_tray_async().await })
}

async fn run_tray_async() -> anyhow::Result<()> {
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

fn run_listen(cfg: Config) -> anyhow::Result<()> {
    let model_path = cfg
        .resolve_model_path()
        .context("resolving whisper model path")?;
    tracing::info!(model = %model_path.display(), "starting listen mode");

    voice_input::injector::verify_available()
        .context("ydotool must be installed and ydotoold running")?;

    // Backend ↔ GTK channel.
    let (overlay_tx, overlay_rx) = overlay::channel();
    // Cloned for the Ctrl+C handler so it can ask the GTK main loop to quit.
    let overlay_tx_for_signal = overlay_tx.clone();

    // Shutdown signal: shared between the main thread (which installs a
    // Ctrl+C handler and also notifies after GTK exits) and the backend
    // (which waits on it via tokio::select!). GTK's own SIGINT handler
    // would otherwise swallow the signal before tokio sees it.
    let shutdown = Arc::new(Notify::new());

    // Spawn the backend thread (owns tokio runtime + ashpd portal + pipeline).
    let cfg_for_backend = cfg.clone();
    let model_path_for_backend = model_path.clone();
    let shutdown_for_backend = shutdown.clone();
    let backend = std::thread::Builder::new()
        .name("voice-input-backend".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime in backend thread");
            if let Err(e) = rt.block_on(run_listen_async(
                cfg_for_backend,
                model_path_for_backend,
                overlay_tx,
                shutdown_for_backend,
            )) {
                tracing::error!(error = %e, "backend exited with error");
            }
        })
        .context("spawning backend thread")?;

    // Run GTK on the OS main thread. The Application's `activate` callback
    // creates the OverlayWindow and attaches the overlay_rx polling loop.
    let app = Application::builder()
        .application_id("com.yetone.VoiceInput")
        .build();

    let overlay_rx_cell = std::cell::RefCell::new(Some(overlay_rx));
    app.connect_activate(move |app| {
        let window = OverlayWindow::new(app);
        // Hidden until backend sends Show.
        window.hide();

        // Take the receiver into a long-lived poll loop.
        let rx = overlay_rx_cell
            .borrow_mut()
            .take()
            .expect("activate called once");
        let window_for_loop = window;
        let app_for_loop = app.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            loop {
                match rx.try_recv() {
                    Ok(UiCmd::Show) => window_for_loop.show(),
                    Ok(UiCmd::Hide) => window_for_loop.hide(),
                    Ok(UiCmd::SetLevel(level)) => window_for_loop.set_level(level),
                    Ok(UiCmd::SetText(text)) => window_for_loop.set_text(&text),
                    Ok(UiCmd::Quit) => {
                        app_for_loop.quit();
                        return gtk4::glib::ControlFlow::Break;
                    }
                    Ok(UiCmd::OpenSettings) => {
                        // TODO(Phase 5.6): Open the Settings dialog.
                        tracing::debug!("OpenSettings received (handler pending)");
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // Backend thread died without sending Quit — clean up.
                        app_for_loop.quit();
                        return gtk4::glib::ControlFlow::Break;
                    }
                }
            }
            gtk4::glib::ControlFlow::Continue
        });
    });

    // Cross-thread Ctrl+C handler: notify the backend and ask the GTK main
    // loop to quit via UiCmd::Quit (the polling closure on the main
    // thread calls app.quit() — we can't capture `Application` here because
    // it's !Send). g_application also installs its own SIGINT handler inside
    // app.run_*; whichever handler ultimately fires, the post-run
    // notify_waiters below is a belt-and-suspenders that guarantees the
    // backend winds down too.
    let shutdown_for_signal = shutdown.clone();
    let _ = ctrlc::set_handler(move || {
        shutdown_for_signal.notify_waiters();
        let _ = overlay_tx_for_signal.send(UiCmd::Quit);
    });

    // GApplication argv parsing tries to treat our clap subcommand (`listen`)
    // as a file path and emits a GLib-GIO CRITICAL when HANDLES_OPEN isn't
    // set, which also suppresses the `activate` signal. Pass empty args so
    // GApplication just calls `activate` and we control its lifecycle.
    let exit_code = app.run_with_args::<&str>(&[]);

    // GTK exited (Ctrl+C handled by GLib, or our handler called app.quit, or
    // some normal exit path). Notify the backend so it stops waiting on
    // portal events, then join it.
    shutdown.notify_waiters();
    let _ = backend.join();

    if exit_code.get() != 0 {
        anyhow::bail!("gtk application exited with code {}", exit_code.get());
    }
    Ok(())
}

async fn run_listen_async(
    cfg: Config,
    model_path: std::path::PathBuf,
    overlay_tx: overlay::UiSender,
    shutdown: Arc<Notify>,
) -> anyhow::Result<()> {
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
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();
    let refiner = voice_input::refiner::LlmRefiner::from_config(&cfg);
    tracing::info!(active = refiner.is_active(), "llm refiner initialized");

    // RMS level fan-out: vad-resample thread → blocking task → overlay channel.
    // We use a bounded crossbeam channel sized for ~1s of audio at 100Hz.
    let (level_tx, level_rx) = crossbeam_channel::bounded::<f32>(128);
    let overlay_tx_for_levels = overlay_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(level) = level_rx.recv() {
            // Stop forwarding when the overlay channel is closed.
            if overlay_tx_for_levels
                .send(UiCmd::SetLevel(level))
                .is_err()
            {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                tracing::info!("shutdown signaled; exiting listen mode");
                break;
            }
            Some(_activated) = activated.next() => {
                if current_pipeline.is_some() {
                    continue;
                }
                tracing::info!("shortcut pressed; starting pipeline");
                match speech::start_pipeline(&model_path, language_hint.clone(), Some(level_tx.clone())) {
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
