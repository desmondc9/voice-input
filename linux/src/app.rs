use std::time::Instant;

use crate::error::ErrorKind;

/// Top-level application state. Transitions are added in later phases;
/// for Phase 0 the type exists so other modules can reference it.
#[derive(Debug, Clone)]
pub enum AppState {
    Idle,
    Listening { started_at: Instant },
    Refining { raw_text: String },
    Injecting { final_text: String },
    Error(ErrorKind),
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_idle() {
        assert!(matches!(AppState::default(), AppState::Idle));
    }

    #[test]
    fn error_variant_carries_kind() {
        let s = AppState::Error(ErrorKind::NoMicrophone);
        match s {
            AppState::Error(k) => assert_eq!(k, ErrorKind::NoMicrophone),
            _ => panic!("expected Error variant"),
        }
    }
}
