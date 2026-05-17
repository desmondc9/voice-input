use voice_activity_detector::VoiceActivityDetector;

use crate::error::{AppError, AppResult};

/// Sample rate of the input audio for VAD. Whisper expects 16 kHz, so we
/// always feed at this rate.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// Window size (number of samples) per VAD inference. Silero is trained
/// at 16 kHz with 512-sample windows (= 32 ms).
const VAD_WINDOW: usize = 512;

/// How many trailing-silence windows close a segment (≈150 ms).
/// 150 ms / 32 ms ≈ 5 windows. Tuned in Phase 7 from 10 (≈300 ms) to
/// reduce the post-release pause; the cost is more aggressive
/// mid-sentence splits, which is acceptable for hold-to-talk dictation
/// where the speaker is expected to talk in short utterances.
const SILENCE_WINDOWS_TO_CLOSE: usize = 5;

/// Max segment length in samples (30 s at 16 kHz = whisper's context window).
const MAX_SEGMENT_SAMPLES: usize = 30 * VAD_SAMPLE_RATE as usize;

/// Speech probability threshold. Silero outputs 0.0–1.0; >0.5 = speech.
const SPEECH_THRESHOLD: f32 = 0.5;

/// Streaming VAD slicer. Feed samples via `push`; the call itself returns
/// any segments that closed during this batch. On shutdown, call `flush`
/// to retrieve any in-progress segment.
pub struct VadSlicer {
    vad: VoiceActivityDetector,
    /// Samples buffered for the next VAD window inference.
    window_buf: Vec<f32>,
    /// Samples accumulated in the current speech segment.
    segment: Vec<f32>,
    /// Number of consecutive silence windows seen.
    silence_count: usize,
    /// True when we've seen at least one speech window in the current segment.
    in_segment: bool,
}

impl VadSlicer {
    pub fn new() -> AppResult<Self> {
        let vad = VoiceActivityDetector::builder()
            .sample_rate(VAD_SAMPLE_RATE as i64)
            .chunk_size(VAD_WINDOW)
            .build()
            .map_err(|e| AppError::WhisperFailed(format!("vad init: {}", e)))?;
        Ok(Self {
            vad,
            window_buf: Vec::with_capacity(VAD_WINDOW),
            segment: Vec::with_capacity(MAX_SEGMENT_SAMPLES),
            silence_count: 0,
            in_segment: false,
        })
    }

    /// Push samples and return any completed segments.
    pub fn push(&mut self, samples: &[f32]) -> AppResult<Vec<Vec<f32>>> {
        let mut completed = Vec::new();
        for &s in samples {
            self.window_buf.push(s);
            if self.window_buf.len() >= VAD_WINDOW {
                let prob = self.vad.predict(self.window_buf.iter().copied());
                let is_speech = prob >= SPEECH_THRESHOLD;

                if is_speech {
                    self.in_segment = true;
                    self.silence_count = 0;
                }

                // Only accumulate into the segment buffer once we're inside
                // a speech segment. Pre-speech silence is discarded so that
                // long stretches without any detected speech don't grow the
                // segment to MAX_SEGMENT_SAMPLES.
                if self.in_segment {
                    self.segment.extend_from_slice(&self.window_buf);
                    if !is_speech {
                        self.silence_count += 1;
                    }
                }
                self.window_buf.clear();

                let should_close = self.in_segment
                    && (self.silence_count >= SILENCE_WINDOWS_TO_CLOSE
                        || self.segment.len() >= MAX_SEGMENT_SAMPLES);
                if should_close {
                    completed.push(std::mem::take(&mut self.segment));
                    self.segment.reserve(MAX_SEGMENT_SAMPLES);
                    self.silence_count = 0;
                    self.in_segment = false;
                }
            }
        }
        Ok(completed)
    }

    /// Force-emit any pending segment (e.g., on shutdown).
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        if self.in_segment && !self.segment.is_empty() {
            self.in_segment = false;
            self.silence_count = 0;
            Some(std::mem::take(&mut self.segment))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate samples for `duration_ms` of either silence or a 1 kHz sine.
    fn samples_for(duration_ms: usize, is_speech: bool) -> Vec<f32> {
        let n = duration_ms * VAD_SAMPLE_RATE as usize / 1000;
        if is_speech {
            (0..n)
                .map(|i| {
                    (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / VAD_SAMPLE_RATE as f32).sin()
                        * 0.5
                })
                .collect()
        } else {
            vec![0.0; n]
        }
    }

    #[test]
    fn instantiates_without_error() {
        let _v = VadSlicer::new().expect("init");
    }

    #[test]
    fn pure_silence_yields_no_segments() {
        let mut v = VadSlicer::new().unwrap();
        let silence = samples_for(2000, false);
        let segments = v.push(&silence).unwrap();
        assert!(segments.is_empty(), "silence produced segments");
    }

    #[test]
    fn flush_returns_none_when_idle() {
        let mut v = VadSlicer::new().unwrap();
        assert!(v.flush().is_none());
    }

    #[test]
    fn long_silence_beyond_max_segment_yields_no_segments() {
        // Regression test: pre-fix, 30+ seconds of pure silence would trigger
        // MAX_SEGMENT_SAMPLES closure and emit a silent segment to whisper.
        let mut v = VadSlicer::new().unwrap();
        let silence = samples_for(35_000, false);
        let segments = v.push(&silence).unwrap();
        assert!(
            segments.is_empty(),
            "pure silence over MAX_SEGMENT_SAMPLES should emit no segments, got {}",
            segments.len()
        );
        assert!(
            v.flush().is_none(),
            "flush should also return None after pure silence"
        );
    }
}
