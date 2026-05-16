/// Compute RMS over `samples` and normalize the resulting dBFS value to
/// the [0, 1] range using the same mapping as the macOS version:
/// `normalized = clamp((dB + 50) / 40, 0, 1)`.
/// dB is computed from `max(rms, 1e-6)` to avoid `log10(0)`.
pub fn rms_normalized(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt().max(1e-6);
    let db = 20.0 * rms.log10();
    ((db + 50.0) / 40.0).clamp(0.0, 1.0)
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Sender;

use crate::error::{AppError, AppResult};

/// Audio buffer + RMS level published by the capture callback.
pub struct AudioChunk {
    /// Samples in the device's native format (interleaved if multi-channel),
    /// converted to f32 in the range [-1, 1].
    pub samples: Vec<f32>,
    /// Native sample rate as reported by cpal.
    pub sample_rate: u32,
    /// Number of channels (1 or 2 typically).
    pub channels: u16,
    /// RMS level of this buffer, normalized to [0, 1] per `rms_normalized`.
    pub level: f32,
}

/// Live audio capture wrapper. Drop to stop the stream.
pub struct Capture {
    _stream: Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Capture {
    /// Open the default input device and start streaming.
    /// Each buffer is sent through `tx` along with the RMS level.
    /// The cpal callback returns immediately after sending; if the channel
    /// is full (downstream backed up), the buffer is silently dropped —
    /// this is the right behavior for real-time audio: never block the
    /// audio thread.
    pub fn start(tx: Sender<AudioChunk>) -> AppResult<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AppError::NoMicrophone("no default input device".into()))?;

        let supported = device
            .default_input_config()
            .map_err(|e| AppError::NoMicrophone(format!("query default config: {}", e)))?;

        let config: StreamConfig = supported.clone().into();
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        tracing::info!(
            device = %device.name().unwrap_or_else(|_| "<unknown>".into()),
            sample_rate,
            channels,
            format = ?supported.sample_format(),
            "opening input stream"
        );

        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, tx, sample_rate, channels)?,
            SampleFormat::I16 => build_stream::<i16>(&device, &config, tx, sample_rate, channels)?,
            SampleFormat::U16 => build_stream::<u16>(&device, &config, tx, sample_rate, channels)?,
            other => {
                return Err(AppError::NoMicrophone(format!(
                    "unsupported sample format: {:?}",
                    other
                )));
            }
        };

        stream
            .play()
            .map_err(|e| AppError::NoMicrophone(format!("starting stream: {}", e)))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
            channels,
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    tx: Sender<AudioChunk>,
    sample_rate: u32,
    channels: u16,
) -> AppResult<Stream>
where
    T: Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let err_fn = |e| tracing::warn!(error = ?e, "cpal stream error");
    let data_fn = move |data: &[T], _: &cpal::InputCallbackInfo| {
        let samples: Vec<f32> = data
            .iter()
            .map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s))
            .collect();
        let level = rms_normalized(&samples);
        let chunk = AudioChunk {
            samples,
            sample_rate,
            channels,
            level,
        };
        // try_send: drop if downstream is backed up; never block audio thread
        let _ = tx.try_send(chunk);
    };

    device
        .build_input_stream(config, data_fn, err_fn, None)
        .map_err(|e| AppError::NoMicrophone(format!("build_input_stream: {}", e)))
}

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Downmix to mono by averaging interleaved channels, then resample to
/// `WHISPER_SAMPLE_RATE` (16 kHz) using a high-quality sinc interpolator.
///
/// Returns the resampled mono f32 samples ready for VAD / whisper.
pub struct Resampler16kMono {
    resampler: SincFixedIn<f32>,
    src_channels: usize,
    /// Carries leftover unprocessed mono samples between calls.
    mono_buf: Vec<f32>,
    chunk_size: usize,
}

impl Resampler16kMono {
    pub fn new(input_rate: u32, channels: u16) -> AppResult<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let chunk_size = 1024_usize;
        let resampler = SincFixedIn::<f32>::new(
            WHISPER_SAMPLE_RATE as f64 / input_rate as f64,
            2.0,
            params,
            chunk_size,
            1, // mono output
        )
        .map_err(|e| AppError::Config(format!("resampler init: {}", e)))?;

        Ok(Self {
            resampler,
            src_channels: channels as usize,
            mono_buf: Vec::with_capacity(chunk_size * 4),
            chunk_size,
        })
    }

    /// Process an interleaved multi-channel buffer at the input rate.
    /// Returns 16 kHz mono samples; may return fewer than expected if
    /// not enough samples have accumulated for a full resampler chunk.
    pub fn process(&mut self, interleaved: &[f32]) -> AppResult<Vec<f32>> {
        // Downmix to mono by averaging channels
        let frames = interleaved.len() / self.src_channels;
        self.mono_buf.reserve(frames);
        for frame in interleaved.chunks_exact(self.src_channels) {
            let avg = frame.iter().sum::<f32>() / self.src_channels as f32;
            self.mono_buf.push(avg);
        }

        let mut output = Vec::new();
        while self.mono_buf.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.mono_buf.drain(..self.chunk_size).collect();
            let resampled = self
                .resampler
                .process(&[chunk], None)
                .map_err(|e| AppError::Config(format!("resample: {}", e)))?;
            output.extend_from_slice(&resampled[0]);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_yields_zero() {
        let silence = vec![0.0_f32; 1024];
        let level = rms_normalized(&silence);
        assert!(level < 0.05, "expected near zero, got {}", level);
    }

    #[test]
    fn full_scale_sine_yields_one() {
        // Full-scale 1 kHz sine at 16 kHz sample rate
        let samples: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16_000.0).sin())
            .collect();
        let level = rms_normalized(&samples);
        assert!(level > 0.9, "expected near one, got {}", level);
    }

    #[test]
    fn empty_returns_zero() {
        assert_eq!(rms_normalized(&[]), 0.0);
    }

    #[test]
    fn quiet_noise_maps_to_low_range() {
        let quiet: Vec<f32> = (0..1024)
            .map(|i| ((i as f32 * 0.1).sin()) * 0.001)
            .collect();
        let level = rms_normalized(&quiet);
        assert!(
            level >= 0.0 && level < 0.2,
            "expected low range, got {}",
            level
        );
    }

    #[test]
    fn resampler_48k_stereo_to_16k_mono_passes_through() {
        let mut r = Resampler16kMono::new(48_000, 2).unwrap();
        // 4800 stereo frames = 100ms at 48kHz
        let interleaved: Vec<f32> = (0..4800)
            .flat_map(|i| {
                let s = (i as f32 * 0.01).sin();
                vec![s, s] // identical left & right
            })
            .collect();
        let out = r.process(&interleaved).unwrap();
        // Roughly 100ms at 16kHz = ~1600 samples — allow some slack for the resampler's internal buffering
        assert!(
            out.len() > 1000 && out.len() < 2000,
            "expected ~1600 output samples, got {}",
            out.len()
        );
        // Output is mono — values should be similar magnitude to input (sin range)
        let max = out.iter().fold(0.0_f32, |a, &b| a.max(b.abs()));
        assert!(max > 0.5 && max < 1.5, "max amplitude unexpected: {}", max);
    }

    #[test]
    fn resampler_44_1k_mono_at_correct_target_rate() {
        let mut r = Resampler16kMono::new(44_100, 1).unwrap();
        let mono: Vec<f32> = (0..22_050).map(|i| (i as f32 * 0.02).sin()).collect(); // ~500ms
        let out = r.process(&mono).unwrap();
        // ~500ms at 16kHz = ~8000 samples; allow ±15% for buffering
        assert!(
            out.len() > 6_500 && out.len() < 9_500,
            "expected ~8000 samples, got {}",
            out.len()
        );
    }
}
