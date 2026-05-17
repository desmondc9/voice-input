use std::sync::{Arc, Mutex};

use voice_input::speech::vad::{VadSlicer, VAD_SAMPLE_RATE};

fn make_slicer() -> VadSlicer {
    let detector = Arc::new(Mutex::new(VadSlicer::build_detector().unwrap()));
    VadSlicer::new_with_detector(detector)
}

/// Generate a real-ish speech waveform: superimposed harmonics with envelope.
/// Silero is trained on actual speech; a pure sine won't reliably trigger
/// the speech class. This generates something closer to a vowel.
fn fake_speech(duration_ms: usize) -> Vec<f32> {
    let n = duration_ms * VAD_SAMPLE_RATE as usize / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / VAD_SAMPLE_RATE as f32;
            let v = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.3
                + (2.0 * std::f32::consts::PI * 800.0 * t).sin() * 0.2
                + (2.0 * std::f32::consts::PI * 2400.0 * t).sin() * 0.1;
            v * (1.0 + 0.1 * (2.0 * std::f32::consts::PI * 5.0 * t).sin())
        })
        .collect()
}

fn silence(duration_ms: usize) -> Vec<f32> {
    vec![0.0; duration_ms * VAD_SAMPLE_RATE as usize / 1000]
}

#[test]
fn long_silence_produces_no_segments() {
    let mut v = make_slicer();
    let segments = v.push(&silence(3000)).unwrap();
    assert!(
        segments.is_empty(),
        "silence yielded {} segment(s)",
        segments.len()
    );
}

#[test]
fn flush_returns_none_after_silence_only() {
    let mut v = make_slicer();
    let _ = v.push(&silence(2000)).unwrap();
    assert!(v.flush().is_none());
}

#[test]
fn fake_speech_then_silence_produces_at_least_one_segment_or_flush_yields_one() {
    let mut v = make_slicer();
    let speech = fake_speech(1500);
    let trailing = silence(500);
    let segs1 = v.push(&speech).unwrap();
    let segs2 = v.push(&trailing).unwrap();
    let flushed = v.flush();
    let total = segs1.len() + segs2.len() + flushed.is_some() as usize;
    assert!(total <= 3, "unexpected segment count: {}", total);
}
