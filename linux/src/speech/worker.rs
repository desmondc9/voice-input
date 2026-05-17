use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
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

/// Spawn a worker thread that uses a pre-loaded `WhisperContext` to
/// transcribe audio slices arriving on `slices_rx`, emitting `String`
/// segments on `text_tx`. Returns a `JoinHandle` so the caller can join
/// on shutdown.
///
/// `ctx` is shared via `Arc` — multiple successive pipelines reuse the
/// same loaded model. Per-inference state is created inside the worker.
///
/// Errors during per-slice inference are logged and skipped — the worker
/// keeps running.
pub fn spawn(
    ctx: Arc<WhisperContext>,
    language_hint: String,
    slices_rx: Receiver<Vec<f32>>,
    text_tx: Sender<String>,
) -> AppResult<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("whisper-worker".into())
        .spawn(move || run(ctx, language_hint, slices_rx, text_tx))
        .map_err(|e| AppError::WhisperFailed(format!("spawn worker thread: {}", e)))?;

    Ok(handle)
}

fn run(
    ctx: Arc<WhisperContext>,
    language_hint: String,
    slices_rx: Receiver<Vec<f32>>,
    text_tx: Sender<String>,
) {
    let mut state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "whisper create_state failed; worker exiting");
            return;
        }
    };

    while let Ok(slice) = slices_rx.recv() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if !language_hint.is_empty() {
            params.set_language(Some(&language_hint));
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
            tracing::info!("whisper worker: text channel closed, exiting");
            return;
        }
    }
    tracing::info!("whisper worker: slice channel closed, exiting");
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
        let (slices_tx, slices_rx) = crossbeam_channel::bounded(1);
        let (text_tx, text_rx) = crossbeam_channel::bounded(1);
        let handle = spawn(ctx, "en".into(), slices_rx, text_tx).unwrap();

        let silence = vec![0.0_f32; 16_000 * 3];
        slices_tx.send(silence).unwrap();

        let _ = text_rx.recv_timeout(std::time::Duration::from_secs(30));
        drop(slices_tx);
        handle.join().unwrap();
    }

    fn dirs_for_test() -> PathBuf {
        directories::ProjectDirs::from("com", "yetone", "voice-input")
            .map(|d| d.data_dir().join("models"))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}
