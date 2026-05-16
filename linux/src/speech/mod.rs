//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.

pub mod vad;
pub mod worker;

use std::path::Path;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Receiver};

use crate::audio::{AudioChunk, Capture, Resampler16kMono};
use crate::error::{AppError, AppResult};

/// Handle returned by `start_pipeline`. Call `join` to perform clean
/// shutdown (closes the audio stream first, then waits for the worker
/// threads). Simply dropping the handle also shuts down — the `Capture`
/// stops on drop and the worker threads will exit when their input
/// channels close.
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    /// Wrapped in `Option` so `join` can drop the audio stream BEFORE
    /// awaiting the VAD thread. Without this, the VAD thread would block
    /// on `audio_rx.recv()` forever because `audio_tx` lives inside the
    /// cpal stream owned by `Capture`.
    capture: Option<Capture>,
    vad_handle: Option<JoinHandle<()>>,
    whisper_handle: Option<JoinHandle<()>>,
}

impl PipelineHandle {
    /// Perform an orderly shutdown:
    /// 1. Drop the audio capture so the cpal stream stops and the
    ///    `audio_tx` Sender is released, allowing the VAD thread's
    ///    `audio_rx.recv()` to return Err.
    /// 2. Join the VAD thread (now able to exit).
    /// 3. The VAD thread, on exit, drops `slice_tx`, which lets the
    ///    whisper worker's `slices_rx.recv()` return Err.
    /// 4. Join the whisper worker thread.
    pub fn join(mut self) {
        self.shutdown_internal();
    }

    /// Shut down (same as `join`), then drain any remaining segments from
    /// `text_rx`. Returns the buffered segments in arrival order.
    ///
    /// Phase 2 uses this after a hotkey release to capture the final
    /// segments before pasting.
    pub fn drain_and_join(mut self) -> Vec<String> {
        self.shutdown_internal();
        // After both worker threads have joined, all senders for text_tx
        // have been dropped. The remaining items in text_rx are exactly
        // the segments produced before shutdown.
        let mut out = Vec::new();
        while let Ok(seg) = self.text_rx.try_recv() {
            out.push(seg);
        }
        out
    }

    fn shutdown_internal(&mut self) {
        drop(self.capture.take());
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.whisper_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        // Idempotent because shutdown_internal uses Option::take.
        self.shutdown_internal();
    }
}

/// Start the audio → resample → VAD → whisper pipeline.
/// Returns a handle including the text receiver.
pub fn start_pipeline(model_path: &Path, language_hint: String) -> AppResult<PipelineHandle> {
    let (audio_tx, audio_rx) = bounded::<AudioChunk>(64);
    let (slice_tx, slice_rx) = bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = bounded::<String>(8);

    let capture = Capture::start(audio_tx)?;
    let input_rate = capture.sample_rate;
    let input_channels = capture.channels;

    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(audio_rx, slice_tx, input_rate, input_channels);
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;

    let whisper_handle = worker::spawn(model_path, language_hint, slice_rx, text_tx)?;

    Ok(PipelineHandle {
        text_rx,
        capture: Some(capture),
        vad_handle: Some(vad_handle),
        whisper_handle: Some(whisper_handle),
    })
}

fn run_vad_resample(
    audio_rx: Receiver<AudioChunk>,
    slice_tx: crossbeam_channel::Sender<Vec<f32>>,
    input_rate: u32,
    input_channels: u16,
) {
    let mut resampler = match Resampler16kMono::new(input_rate, input_channels) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "resampler init failed");
            return;
        }
    };
    let mut slicer = match vad::VadSlicer::new() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "vad init failed");
            return;
        }
    };

    while let Ok(chunk) = audio_rx.recv() {
        let mono16k = match resampler.process(&chunk.samples) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "resample failed");
                continue;
            }
        };
        let segments = match slicer.push(&mono16k) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "vad push failed");
                continue;
            }
        };
        for seg in segments {
            if slice_tx.send(seg).is_err() {
                tracing::info!("vad: downstream closed, exiting");
                return;
            }
        }
    }

    if let Some(final_segment) = slicer.flush() {
        let _ = slice_tx.send(final_segment);
    }
    tracing::info!("vad: audio channel closed, exiting");
}
