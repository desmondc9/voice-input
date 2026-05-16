use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use ksni::TrayMethods;
use tokio::sync::Notify;
use voice_input::{
    cli::{Cli, Command},
    config::Config,
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

    let pipeline = voice_input::speech::start_pipeline(&model_path, cfg.language_hint.clone())
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

    pipeline.join();
    tracing::info!(
        "pipeline shutdown complete; transcribed {} segments",
        segment_count
    );
    Ok(())
}

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
                    let segments = pipeline.drain_and_join();
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
                tracing::warn!("portal streams closed; exiting");
                break;
            }
        }
    }

    Ok(())
}
