use std::path::Path;
use std::sync::Arc;
use std::thread;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::{AppError, AppResult};

/// Load a whisper model from disk into a `WhisperContext`. Expensive
/// (487 MB read + parse for `small`); call once and share via `Arc` so
/// successive dictations don't pay this cost again.
pub fn load_whisper_context(model_path: &Path) -> AppResult<WhisperContext> {
    if !model_path.exists() {
        return Err(AppError::ModelMissing {
            path: model_path.to_path_buf(),
        });
    }
    let path_str = model_path.to_string_lossy().into_owned();
    WhisperContext::new_with_params(&path_str, WhisperContextParameters::default())
        .map_err(|e| AppError::WhisperFailed(format!("load model {}: {}", path_str, e)))
}

/// Long-lived whisper worker that owns a `WhisperState` across many
/// dictations. Built once at app startup; each dictation calls
/// `start_session` with fresh per-dictation channels. The worker thread
/// itself never exits until the user issues `shutdown` (or the cmd
/// channel is closed).
pub struct PersistentWhisperWorker {
    cmd_tx: crossbeam_channel::Sender<WorkerCmd>,
    handle: Option<thread::JoinHandle<()>>,
}

enum WorkerCmd {
    Run {
        language_hint: String,
        slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
        text_tx: crossbeam_channel::Sender<String>,
    },
    Shutdown,
}

impl PersistentWhisperWorker {
    pub fn spawn(ctx: Arc<WhisperContext>) -> AppResult<Self> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<WorkerCmd>(1);
        let handle = thread::Builder::new()
            .name("whisper-worker-persistent".into())
            .spawn(move || run_persistent(ctx, cmd_rx))
            .map_err(|e| AppError::WhisperFailed(format!("spawn persistent worker: {}", e)))?;
        Ok(Self {
            cmd_tx,
            handle: Some(handle),
        })
    }

    /// Enqueue one dictation's worth of inference. Non-blocking: returns
    /// `WhisperFailed("worker busy")` if a session is already in flight
    /// (the channel is bounded(1)) instead of parking the caller's
    /// thread. The current call site in `start_pipeline` is reached via
    /// the tokio backend's `select!` loop, so blocking here would freeze
    /// the entire activation/deactivation/shutdown event handling.
    pub fn start_session(
        &self,
        language_hint: String,
        slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
        text_tx: crossbeam_channel::Sender<String>,
    ) -> AppResult<()> {
        self.cmd_tx
            .try_send(WorkerCmd::Run {
                language_hint,
                slice_rx,
                text_tx,
            })
            .map_err(|e| match e {
                crossbeam_channel::TrySendError::Full(_) => {
                    AppError::WhisperFailed("whisper worker busy: session already in flight".into())
                }
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    AppError::WhisperFailed("whisper worker thread exited".into())
                }
            })
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PersistentWhisperWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_persistent(ctx: Arc<WhisperContext>, cmd_rx: crossbeam_channel::Receiver<WorkerCmd>) {
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "persistent whisper worker: create_state failed");
            return;
        }
    };
    tracing::info!("persistent whisper worker: state ready");

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCmd::Run {
                language_hint,
                slice_rx,
                text_tx,
            } => {
                run_session(&mut state, &language_hint, slice_rx, text_tx);
            }
            WorkerCmd::Shutdown => break,
        }
    }
    tracing::info!("persistent whisper worker: exiting");
}

fn run_session(
    state: &mut whisper_rs::WhisperState,
    language_hint: &str,
    slice_rx: crossbeam_channel::Receiver<Vec<f32>>,
    text_tx: crossbeam_channel::Sender<String>,
) {
    while let Ok(slice) = slice_rx.recv() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if !language_hint.is_empty() {
            params.set_language(Some(language_hint));
        }
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        if let Err(e) = state.full(params, &slice) {
            tracing::warn!(error = %e, samples = slice.len(), "whisper inference failed; skipping slice");
            continue;
        }

        let n_segments = match state.full_n_segments() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "full_n_segments failed");
                continue;
            }
        };
        let mut combined = String::new();
        for i in 0..n_segments {
            match state.full_get_segment_text(i) {
                Ok(text) => {
                    if !combined.is_empty() {
                        combined.push(' ');
                    }
                    combined.push_str(text.trim());
                }
                Err(e) => tracing::warn!(error = %e, segment = i, "get_segment_text failed"),
            }
        }
        let trimmed = combined.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        if text_tx.send(trimmed).is_err() {
            tracing::info!("whisper session: text channel closed, exiting session");
            return;
        }
    }
    tracing::info!("whisper session: slice channel closed, session done");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_model_path_returns_model_missing_error() {
        let path = PathBuf::from("/nonexistent/ggml-tiny.bin");
        match load_whisper_context(&path) {
            Err(AppError::ModelMissing { path: p }) => {
                assert!(p.to_string_lossy().contains("nonexistent"))
            }
            Err(other) => panic!("expected ModelMissing, got {:?}", other),
            Ok(_) => panic!("expected ModelMissing error, got Ok(WhisperContext)"),
        }
    }

    /// Real inference test — requires a downloaded whisper model.
    /// Run with: cargo test --lib -- --ignored
    #[test]
    #[ignore]
    fn transcribes_silence_to_empty_or_short_text() {
        let model_path = std::env::var("VOICE_INPUT_MODEL_PATH")
            .or_else(|_| {
                Ok::<_, std::env::VarError>(
                    dirs_for_test()
                        .join("ggml-tiny.bin")
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .unwrap();
        let path = PathBuf::from(&model_path);
        if !path.exists() {
            eprintln!("skipping: model not at {}", model_path);
            return;
        }
        let ctx = Arc::new(load_whisper_context(&path).unwrap());
        let mut worker = PersistentWhisperWorker::spawn(ctx).unwrap();
        let (slices_tx, slices_rx) = crossbeam_channel::bounded(1);
        let (text_tx, text_rx) = crossbeam_channel::bounded(1);
        worker
            .start_session("en".into(), slices_rx, text_tx)
            .unwrap();

        let silence = vec![0.0_f32; 16_000 * 3];
        slices_tx.send(silence).unwrap();
        drop(slices_tx);

        let _ = text_rx.recv_timeout(std::time::Duration::from_secs(30));
        worker.shutdown();
    }

    fn dirs_for_test() -> PathBuf {
        directories::ProjectDirs::from("com", "yetone", "voice-input")
            .map(|d| d.data_dir().join("models"))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}
