use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    NoMicrophone,
    ModelMissing,
    WhisperFailed,
    PortalRevoked,
    YdotoolMissing,
    NetworkError,
    Config,
    Io,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("no microphone available: {0}")]
    NoMicrophone(String),

    #[error("whisper model file missing at {path}")]
    ModelMissing { path: PathBuf },

    #[error("whisper inference failed: {0}")]
    WhisperFailed(String),

    #[error("global shortcut session revoked")]
    PortalRevoked,

    #[error("ydotool unavailable: {0}")]
    YdotoolMissing(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            AppError::NoMicrophone(_) => ErrorKind::NoMicrophone,
            AppError::ModelMissing { .. } => ErrorKind::ModelMissing,
            AppError::WhisperFailed(_) => ErrorKind::WhisperFailed,
            AppError::PortalRevoked => ErrorKind::PortalRevoked,
            AppError::YdotoolMissing(_) => ErrorKind::YdotoolMissing,
            AppError::NetworkError(_) => ErrorKind::NetworkError,
            AppError::Config(_) => ErrorKind::Config,
            AppError::Io(_) => ErrorKind::Io,
        }
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_mapping_is_exhaustive() {
        assert_eq!(AppError::NoMicrophone("x".into()).kind(), ErrorKind::NoMicrophone);
        assert_eq!(
            AppError::ModelMissing { path: "/tmp/x".into() }.kind(),
            ErrorKind::ModelMissing
        );
        assert_eq!(AppError::WhisperFailed("x".into()).kind(), ErrorKind::WhisperFailed);
        assert_eq!(AppError::PortalRevoked.kind(), ErrorKind::PortalRevoked);
        assert_eq!(AppError::YdotoolMissing("x".into()).kind(), ErrorKind::YdotoolMissing);
        assert_eq!(AppError::NetworkError("x".into()).kind(), ErrorKind::NetworkError);
        assert_eq!(AppError::Config("x".into()).kind(), ErrorKind::Config);
        let io = AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "x"));
        assert_eq!(io.kind(), ErrorKind::Io);
    }

    #[test]
    fn display_includes_context() {
        let err = AppError::NoMicrophone("default device missing".into());
        assert!(err.to_string().contains("default device missing"));
    }

    #[test]
    fn io_error_auto_converts() {
        fn read() -> AppResult<String> {
            std::fs::read_to_string("/nonexistent/path/that/does/not/exist")?;
            Ok(String::new())
        }
        let err = read().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Io);
    }
}
