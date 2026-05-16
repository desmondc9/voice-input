use std::io::Read;
use std::process::Command;
use std::time::Duration;

use wl_clipboard_rs::copy::{MimeType as CopyMime, Options as CopyOptions, Source};
use wl_clipboard_rs::paste::{
    get_contents, ClipboardType, MimeType as PasteMime, Seat as PasteSeat,
};

use crate::error::{AppError, AppResult};

/// Verify that `ydotool` is invocable. Called once at startup of `listen`
/// mode so the user gets a clear error if the binary is missing.
///
/// `ydotool` (no args) prints its usage banner and exits 0 on supported
/// versions; we only need to confirm the spawn succeeded (binary on PATH).
/// We do NOT probe `ydotoold` here — if the daemon is down, the first
/// paste attempt fails with a clear error.
pub fn verify_available() -> AppResult<()> {
    let output = Command::new("ydotool")
        .output()
        .map_err(|e| AppError::YdotoolMissing(format!("ydotool not found on PATH: {e}")))?;
    let banner = String::from_utf8_lossy(&output.stdout);
    tracing::info!(
        banner = %banner.lines().next().unwrap_or("<empty>"),
        "ydotool available"
    );
    Ok(())
}

/// Paste `text` into whatever app currently has keyboard focus:
/// 1. Snapshot the current clipboard text (if any).
/// 2. Write `text` to the clipboard.
/// 3. Invoke `ydotool key 29:1 47:1 47:0 29:0` (Ctrl-down V-down V-up Ctrl-up).
/// 4. Sleep 500 ms so the paste latches.
/// 5. Restore the original clipboard.
///
/// Blocking by design — the caller (Phase 2 `run_listen` release handler)
/// runs this from `tokio::task::spawn_blocking` or a sync section so the
/// async runtime isn't stalled.
pub fn inject_text(text: &str) -> AppResult<()> {
    if text.is_empty() {
        return Ok(());
    }

    let saved = snapshot_clipboard().ok();

    write_clipboard(text)?;

    let status = Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"])
        .status()
        .map_err(|e| AppError::YdotoolMissing(format!("running ydotool key: {e}")))?;
    if !status.success() {
        return Err(AppError::YdotoolMissing(format!(
            "ydotool key exited with status {status}"
        )));
    }

    std::thread::sleep(Duration::from_millis(500));

    if let Some(original) = saved {
        // Best-effort restore; log on failure but don't propagate.
        if let Err(e) = write_clipboard(&original) {
            tracing::warn!(error = %e, "failed to restore original clipboard");
        }
    }

    Ok(())
}

fn snapshot_clipboard() -> AppResult<String> {
    let (mut pipe, _mime) =
        get_contents(ClipboardType::Regular, PasteSeat::Unspecified, PasteMime::Text)
            .map_err(|e| AppError::Config(format!("clipboard snapshot: {e}")))?;
    let mut buf = String::new();
    pipe.read_to_string(&mut buf)
        .map_err(AppError::Io)?;
    Ok(buf)
}

fn write_clipboard(text: &str) -> AppResult<()> {
    let mut opts = CopyOptions::new();
    opts.foreground(false); // serve in a forked process so we don't block
    opts.copy(
        Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
        CopyMime::Specific("text/plain;charset=utf-8".into()),
    )
    .map_err(|e| AppError::Config(format!("clipboard write: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_empty_text_is_noop() {
        // Even without ydotool/Wayland available, the empty-text early return
        // must not call any external command.
        assert!(inject_text("").is_ok());
    }

    /// Verify the public function signature is `pub fn verify_available() -> AppResult<()>`.
    /// Real exec is gated behind #[ignore] since not all environments have ydotool.
    #[test]
    #[ignore]
    fn verify_available_when_ydotool_present() {
        // Run with: cargo test --lib injector -- --ignored
        verify_available().expect("ydotool should be available");
    }
}
