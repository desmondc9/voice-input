use voice_input::audio::rms_normalized;

#[test]
fn monotonic_increase_with_amplitude() {
    let amplitudes = [0.001_f32, 0.01, 0.05, 0.2, 0.7];
    let mut prev = -1.0_f32;
    for amp in amplitudes {
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32).sin() * amp).collect();
        let level = rms_normalized(&samples);
        assert!(
            level >= prev,
            "level for amplitude {} ({}) should be >= previous ({})",
            amp,
            level,
            prev
        );
        prev = level;
    }
}

#[test]
fn output_is_bounded_zero_to_one() {
    for amplitude in [-100.0_f32, -1.0, 0.0, 0.5, 1.0, 100.0] {
        let samples: Vec<f32> = vec![amplitude; 512];
        let level = rms_normalized(&samples);
        assert!(
            (0.0..=1.0).contains(&level),
            "level {} for amplitude {} out of bounds",
            level,
            amplitude
        );
    }
}
