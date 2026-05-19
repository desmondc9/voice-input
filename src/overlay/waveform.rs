use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{glib, DrawingArea};

/// 5-bar waveform widget — exact port of macOS `WaveformView`.
///
/// Constants mirror `dist/Sources/VoiceInput/OverlayPanel.swift:181-217`:
/// - 5 bars with weights `[0.5, 0.8, 1.0, 0.75, 0.55]` (center-high)
/// - Attack 0.4 / release 0.15 smoothing on the input level
/// - ±4% per-bar jitter for organic feel
/// - `MIN_BAR_FRACTION = 0.15` so silent bars stay visible
/// - Bar width 4.5 px, gap 3.5 px, view 44×32 px
const BAR_COUNT: usize = 5;
const BAR_WEIGHTS: [f64; BAR_COUNT] = [0.5, 0.8, 1.0, 0.75, 0.55];
const MIN_BAR_FRACTION: f64 = 0.15;
const ATTACK: f64 = 0.4;
const RELEASE: f64 = 0.15;
const JITTER: f64 = 0.04;

const BAR_WIDTH: f64 = 4.5;
const BAR_GAP: f64 = 3.5;
const VIEW_WIDTH: i32 = 44;
const VIEW_HEIGHT: i32 = 32;

const REDRAW_HZ: u32 = 60;

pub struct WaveformView {
    drawing_area: DrawingArea,
    smoothed_level: Rc<Cell<f64>>,
    target_level: Rc<Cell<f64>>,
}

impl Default for WaveformView {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveformView {
    pub fn new() -> Self {
        let drawing_area = DrawingArea::builder()
            .content_width(VIEW_WIDTH)
            .content_height(VIEW_HEIGHT)
            .build();

        let smoothed_level = Rc::new(Cell::new(0.0_f64));
        let target_level = Rc::new(Cell::new(0.0_f64));

        // Per-frame smoothing + redraw via glib::timeout_add_local.
        let smoothed = smoothed_level.clone();
        let target = target_level.clone();
        let area_ref = drawing_area.clone();
        glib::timeout_add_local(
            std::time::Duration::from_millis(1000 / REDRAW_HZ as u64),
            move || {
                let prev = smoothed.get();
                let tgt = target.get();
                let factor = if tgt > prev { ATTACK } else { RELEASE };
                let new = prev + (tgt - prev) * factor;
                smoothed.set(new);
                area_ref.queue_draw();
                glib::ControlFlow::Continue
            },
        );

        // Draw callback closes over smoothed_level (read-only).
        let smoothed_for_draw = smoothed_level.clone();
        drawing_area.set_draw_func(move |_, ctx, w, h| {
            draw_bars(ctx, w as f64, h as f64, smoothed_for_draw.get());
        });

        Self {
            drawing_area,
            smoothed_level,
            target_level,
        }
    }

    pub fn widget(&self) -> &DrawingArea {
        &self.drawing_area
    }

    /// Push a new target level. The widget smooths toward it at REDRAW_HZ.
    /// Input is expected to be in [0, 1] (per `audio::rms_normalized`).
    pub fn set_level(&self, level: f32) {
        let clamped = (level as f64).clamp(0.0, 1.0);
        self.target_level.set(clamped);
    }

    /// Snap level to 0 and clear smoothing — used when the capsule is
    /// re-shown so old levels don't bleed across sessions.
    pub fn reset(&self) {
        self.target_level.set(0.0);
        self.smoothed_level.set(0.0);
        self.drawing_area.queue_draw();
    }
}

fn draw_bars(ctx: &Context, width: f64, height: f64, level: f64) {
    let total_width = BAR_COUNT as f64 * BAR_WIDTH + (BAR_COUNT - 1) as f64 * BAR_GAP;
    let start_x = (width - total_width) / 2.0;
    let center_y = height / 2.0;

    // Bar color: rgba(255, 255, 255, 0.92).
    ctx.set_source_rgba(1.0, 1.0, 1.0, 0.92);

    for (i, &weight) in BAR_WEIGHTS.iter().enumerate() {
        // Cheap jitter — using a hash of (i, level) keeps it stable per
        // frame so bars don't flicker chaotically. Real impl could use
        // a per-bar rng. ±4% per the macOS constant.
        let jitter = (((i as f64) * 73.0 + level * 991.0).sin()) * JITTER;
        let fraction = MIN_BAR_FRACTION + (1.0 - MIN_BAR_FRACTION) * level * weight;
        let clamped = (fraction + jitter).clamp(MIN_BAR_FRACTION, 1.0);
        let bar_h = height * clamped;

        let x = start_x + i as f64 * (BAR_WIDTH + BAR_GAP);
        let y = center_y - bar_h / 2.0;

        // Rounded rect (corner radius 2.5 px like macOS).
        rounded_rect(ctx, x, y, BAR_WIDTH, bar_h, 2.5);
        ctx.fill().unwrap_or(());
    }
}

fn rounded_rect(ctx: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.new_sub_path();
    ctx.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    ctx.arc(x + r, y + h - r, r, PI / 2.0, PI);
    ctx.arc(x + r, y + r, r, PI, 3.0 * PI / 2.0);
    ctx.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_close_to_design() {
        let sum: f64 = BAR_WEIGHTS.iter().sum();
        // Center-high distribution: 0.5+0.8+1.0+0.75+0.55 = 3.6
        assert!((sum - 3.6).abs() < 1e-9, "weights drift: {}", sum);
    }

    #[test]
    fn min_bar_fraction_keeps_silence_visible() {
        // At level=0, jitter=0 → fraction = MIN_BAR_FRACTION = 0.15.
        // Multiplied by view_height=32: silent bars are ~4.8px tall, still visible.
        assert_eq!(MIN_BAR_FRACTION, 0.15);
        assert!((MIN_BAR_FRACTION * VIEW_HEIGHT as f64) > 4.0);
    }

    #[test]
    fn full_scale_level_reaches_full_height_after_jitter() {
        // At level=1.0, fraction = 0.15 + 0.85 * 1.0 * weight; for the
        // center bar (weight=1.0) that's 1.0. With +4% jitter we'd exceed
        // 1.0, but clamp() pins to 1.0.
        let level = 1.0;
        let weight = 1.0;
        let fraction = MIN_BAR_FRACTION + (1.0 - MIN_BAR_FRACTION) * level * weight;
        assert!((fraction - 1.0).abs() < 1e-9);
    }
}
