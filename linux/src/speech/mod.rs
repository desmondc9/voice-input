//! Speech pipeline: VAD slicing + whisper transcription.
//!
//! These modules deliberately use `std::thread` + `crossbeam_channel`
//! rather than tokio tasks. Phase 3 will move the GTK4 event loop onto
//! the main thread; keeping the speech pipeline runtime-agnostic means
//! we don't have to rewrite it then.

pub mod vad;
pub mod worker;
