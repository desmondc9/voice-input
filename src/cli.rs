use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "voice-input", version, about = "Wayland-native voice input")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the audio capture + whisper transcription pipeline and print
    /// segments to stdout. No tray, no UI. Press Ctrl+C to stop.
    Transcribe,
    /// Run as a foreground daemon: hold the configured global hotkey to
    /// record, release to paste the transcribed text into the focused
    /// application. First run prompts the XDG portal to bind a shortcut.
    Listen,
}
