//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules use `std::thread` + `crossbeam_channel`, not tokio
//! tasks. `voice-input listen` (Phase 2+) runs GTK4 on the OS main
//! thread (a hard GTK4 requirement) and the tokio runtime in a
//! background `std::thread`; the speech pipeline lives entirely
//! beneath that backend thread and is unaffected by the GTK/tokio
//! split. See `main::run_listen` for the wiring.
//!
//! `start_pipeline` returns `(Capture, PipelineHandle)` as a tuple:
//! `Capture` holds the cpal stream and is `!Send`, so it stays on
//! the calling thread. `PipelineHandle` is `Send` and can be moved
//! into `spawn_blocking` for the worker-thread joins on shutdown.

pub mod vad;
pub mod worker;

use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Receiver};

use crate::audio::{AudioChunk, Capture, Resampler16kMono};
use crate::error::{AppError, AppResult};

/// Send-friendly handle to the speech pipeline worker threads.
///
/// `PipelineHandle` deliberately does NOT own the `Capture` audio stream
/// (which is `!Send` because `cpal::Stream` is `!Send`). The caller of
/// `start_pipeline` receives a `(Capture, PipelineHandle)` tuple and must
/// hold Capture on the thread where the pipeline was started; this lets
/// the PipelineHandle be moved into `tokio::task::spawn_blocking` while
/// the caller drops Capture on the original thread to unblock the VAD
/// thread's `audio_rx.recv()`.
pub struct PipelineHandle {
    pub text_rx: Receiver<String>,
    vad_handle: Option<JoinHandle<()>>,
}

impl PipelineHandle {
    /// Join the vad thread, then drain `text_rx`.
    ///
    /// VAD dropping `slice_tx` signals the persistent whisper worker's
    /// session loop to exit, which causes `text_tx` to be dropped, which
    /// terminates the `text_rx` recv loop below.
    ///
    /// IMPORTANT: the caller must drop the corresponding `Capture` BEFORE
    /// calling this (typically via `drop(capture)` on the async thread,
    /// then `spawn_blocking(move || handle.join_remaining())`). Without
    /// dropping Capture first, this will block forever because the VAD
    /// thread is waiting for `audio_rx` to close.
    pub fn join_remaining(mut self) -> Vec<String> {
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
        // VAD dropped slice_tx → persistent whisper worker's session
        // loop exits → drops text_tx → recv loop here terminates.
        let mut out = Vec::new();
        while let Ok(seg) = self.text_rx.recv() {
            out.push(seg);
        }
        out
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        // If `join_remaining` wasn't called, ensure the thread still gets
        // joined on Drop. This won't deadlock as long as the corresponding
        // Capture has been dropped — if it hasn't, the caller has a bug
        // and will see this hang in their logs.
        if let Some(h) = self.vad_handle.take() {
            let _ = h.join();
        }
    }
}

/// Start the audio → resample → VAD → whisper pipeline.
/// Returns `(Capture, PipelineHandle)` where Capture must be held on the
/// spawning thread (it's !Send) and PipelineHandle can be moved to spawn_blocking.
///
/// If `level_tx` is provided, RMS levels from each incoming `AudioChunk`
/// are forwarded via `try_send` (levels are lossy; dropped if the consumer
/// is slow).
pub fn start_pipeline(
    whisper_worker: &worker::PersistentWhisperWorker,
    vad_detector: std::sync::Arc<
        std::sync::Mutex<voice_activity_detector::VoiceActivityDetector>,
    >,
    language_hint: String,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
) -> AppResult<(Capture, PipelineHandle)> {
    let (audio_tx, audio_rx) = bounded::<AudioChunk>(64);
    let (slice_tx, slice_rx) = bounded::<Vec<f32>>(8);
    let (text_tx, text_rx) = bounded::<String>(64);

    let capture = Capture::start(audio_tx)?;
    let input_rate = capture.sample_rate;
    let input_channels = capture.channels;

    let vad_handle = std::thread::Builder::new()
        .name("vad-resample".into())
        .spawn(move || {
            run_vad_resample(
                audio_rx,
                slice_tx,
                input_rate,
                input_channels,
                level_tx,
                vad_detector,
            );
        })
        .map_err(|e| AppError::Config(format!("spawn vad thread: {}", e)))?;

    whisper_worker.start_session(language_hint, slice_rx, text_tx)?;

    Ok((
        capture,
        PipelineHandle {
            text_rx,
            vad_handle: Some(vad_handle),
        },
    ))
}

fn run_vad_resample(
    audio_rx: Receiver<AudioChunk>,
    slice_tx: crossbeam_channel::Sender<Vec<f32>>,
    input_rate: u32,
    input_channels: u16,
    level_tx: Option<crossbeam_channel::Sender<f32>>,
    vad_detector: std::sync::Arc<
        std::sync::Mutex<voice_activity_detector::VoiceActivityDetector>,
    >,
) {
    let mut resampler = match Resampler16kMono::new(input_rate, input_channels) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "resampler init failed");
            return;
        }
    };
    let mut slicer = vad::VadSlicer::new_with_detector(vad_detector);

    while let Ok(chunk) = audio_rx.recv() {
        // Fan out the RMS level to the overlay if subscribed.
        if let Some(tx) = level_tx.as_ref() {
            // try_send: if the consumer is slow, drop the level update.
            // Levels are inherently lossy anyway.
            let _ = tx.try_send(chunk.level);
        }
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
