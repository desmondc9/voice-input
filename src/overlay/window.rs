use gtk4::prelude::*;
use gtk4::{Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Label, Orientation};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::waveform::WaveformView;

const CAPSULE_WIDTH: i32 = 360;
const CAPSULE_HEIGHT: i32 = 56;
const CAPSULE_MARGIN_BOTTOM: i32 = 56;

/// CSS applied to the overlay capsule. Linux-native styling: solid dark
/// alpha background, soft inner border, drop shadow. No blur — that would
/// require compositor-specific protocols (KWin blur effect) and isn't
/// portable. See brainstorm decision in plans/2026-05-15-voice-input-linux/design.md.
const CAPSULE_CSS: &str = r#"
window.voice-input-overlay {
    background: transparent;
}
window.voice-input-overlay > .capsule {
    background-color: rgba(28, 28, 36, 0.92);
    border-radius: 28px;
    border: 1px solid rgba(255, 255, 255, 0.10);
    padding: 8px 24px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
}
window.voice-input-overlay .overlay-label {
    color: rgba(255, 255, 255, 0.92);
    font-size: 15px;
    font-weight: 500;
    padding-left: 14px;
}
"#;

pub struct OverlayWindow {
    window: ApplicationWindow,
    label: Label,
    waveform: WaveformView,
}

impl OverlayWindow {
    pub fn new(app: &Application) -> Self {
        // Install the CSS once globally per gtk4 docs — adding multiple
        // providers stacks them; using one is fine.
        let provider = CssProvider::new();
        provider.load_from_string(CAPSULE_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(CAPSULE_WIDTH)
            .default_height(CAPSULE_HEIGHT)
            .resizable(false)
            .decorated(false)
            .build();
        window.add_css_class("voice-input-overlay");

        // layer-shell setup: bottom-center, no exclusive zone, no keyboard focus.
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, CAPSULE_MARGIN_BOTTOM);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_exclusive_zone(-1);

        // Layout: [waveform] [label]
        let capsule = GtkBox::new(Orientation::Horizontal, 14);
        capsule.add_css_class("capsule");
        capsule.set_halign(Align::Center);
        capsule.set_valign(Align::Center);

        let waveform = WaveformView::new();
        capsule.append(waveform.widget());

        let label = Label::new(Some("Listening…"));
        label.add_css_class("overlay-label");
        label.set_halign(Align::Start);
        capsule.append(&label);

        window.set_child(Some(&capsule));

        Self {
            window,
            label,
            waveform,
        }
    }

    pub fn show(&self) {
        self.label.set_text("Listening…");
        self.waveform.reset();
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn set_text(&self, text: &str) {
        self.label.set_text(text);
    }

    pub fn set_level(&self, level: f32) {
        self.waveform.set_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_dimensions_match_design() {
        // Pinned constants — Phase 5 might tune these, but accidental
        // drift should be a deliberate decision.
        assert_eq!(CAPSULE_WIDTH, 360);
        assert_eq!(CAPSULE_HEIGHT, 56);
        assert_eq!(CAPSULE_MARGIN_BOTTOM, 56);
    }
}
