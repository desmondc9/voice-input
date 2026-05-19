use ashpd::desktop::global_shortcuts::{Activated, Deactivated, GlobalShortcuts, NewShortcut};
use ashpd::desktop::Session;
use futures_util::stream::Stream;

use crate::error::{AppError, AppResult};

/// The single shortcut id used by Phase 2.
pub const SHORTCUT_ID: &str = "toggle_recording";

/// Wraps an ashpd `GlobalShortcuts` session and exposes activated /
/// deactivated event streams. Drop the handle to end the session.
pub struct HotkeyHandle {
    proxy: GlobalShortcuts,
    _session: Session<GlobalShortcuts>,
}

impl HotkeyHandle {
    /// Create a new portal session and ensure a shortcut is bound.
    /// On first run, the portal shows a binding dialog; on subsequent runs
    /// (when the portal remembers the binding by app id) it should not.
    pub async fn create() -> AppResult<HotkeyHandle> {
        let proxy = GlobalShortcuts::new()
            .await
            .map_err(|e| AppError::Config(format!("portal create proxy: {e}")))?;
        let session = proxy
            .create_session(Default::default())
            .await
            .map_err(|e| AppError::Config(format!("portal create session: {e}")))?;

        let shortcut = NewShortcut::new(SHORTCUT_ID, "Hold to dictate").preferred_trigger("CTRL_R");

        let response = proxy
            .bind_shortcuts(&session, &[shortcut], None, Default::default())
            .await
            .map_err(|e| AppError::Config(format!("portal bind: {e}")))?
            .response()
            .map_err(|e| AppError::Config(format!("portal bind response: {e}")))?;

        tracing::info!(
            shortcuts = ?response.shortcuts().iter().map(|s| s.id().to_string()).collect::<Vec<_>>(),
            "portal bound shortcuts"
        );

        Ok(HotkeyHandle {
            proxy,
            _session: session,
        })
    }

    /// Stream of "pressed" events (one per key-down).
    pub async fn activated(&self) -> AppResult<impl Stream<Item = Activated> + '_> {
        self.proxy
            .receive_activated()
            .await
            .map_err(|e| AppError::Config(format!("portal activated stream: {e}")))
    }

    /// Stream of "released" events (one per key-up).
    pub async fn deactivated(&self) -> AppResult<impl Stream<Item = Deactivated> + '_> {
        self.proxy
            .receive_deactivated()
            .await
            .map_err(|e| AppError::Config(format!("portal deactivated stream: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_id_constant_is_stable() {
        // Phase 5 may add more shortcut ids; this test pins the toggle_recording
        // id so subsequent phases don't accidentally rename it (which would
        // break user-bound shortcuts on upgrade).
        assert_eq!(SHORTCUT_ID, "toggle_recording");
    }
}
