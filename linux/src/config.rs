use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub language_hint: String,
    pub llm_enabled: bool,
    pub llm_api_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub whisper_model_size: String,
    pub whisper_model_path: Option<PathBuf>,
    pub shortcut_handle: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language_hint: "zh".to_string(),
            llm_enabled: false,
            llm_api_base_url: "https://api.openai.com/v1".to_string(),
            llm_api_key: String::new(),
            llm_model: "gpt-4o-mini".to_string(),
            whisper_model_size: "small".to_string(),
            whisper_model_path: None,
            shortcut_handle: None,
        }
    }
}

impl Config {
    pub fn config_path() -> AppResult<PathBuf> {
        let dirs = directories::ProjectDirs::from("com", "yetone", "voice-input")
            .ok_or_else(|| AppError::Config("cannot resolve XDG config dir".into()))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load() -> AppResult<Self> {
        Self::load_from(&Self::config_path()?)
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&Self::config_path()?)
    }

    pub fn load_from(path: &Path) -> AppResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content)
            .map_err(|e| AppError::Config(format!("parse {}: {}", path.display(), e)))
    }

    pub fn save_to(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("serialize: {}", e)))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Resolve the whisper model file path:
    /// 1. `$VOICE_INPUT_MODEL_PATH` env var if set
    /// 2. `whisper_model_path` field if `Some`
    /// 3. `~/.local/share/voice-input/models/ggml-{whisper_model_size}.bin`
    pub fn resolve_model_path(&self) -> AppResult<PathBuf> {
        if let Ok(env) = std::env::var("VOICE_INPUT_MODEL_PATH") {
            if !env.is_empty() {
                return Ok(PathBuf::from(env));
            }
        }
        if let Some(ref p) = self.whisper_model_path {
            return Ok(p.clone());
        }
        let dirs = directories::ProjectDirs::from("com", "yetone", "voice-input")
            .ok_or_else(|| AppError::Config("cannot resolve XDG data dir".into()))?;
        Ok(dirs
            .data_dir()
            .join("models")
            .join(format!("ggml-{}.bin", self.whisper_model_size)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_sensible_values() {
        let cfg = Config::default();
        assert_eq!(cfg.language_hint, "zh");
        assert_eq!(cfg.llm_api_base_url, "https://api.openai.com/v1");
        assert!(!cfg.llm_enabled);
        assert_eq!(cfg.whisper_model_size, "small");
        assert!(cfg.whisper_model_path.is_none());
        assert!(cfg.shortcut_handle.is_none());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn invalid_toml_returns_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not = valid [[ toml").unwrap();
        let err = Config::load_from(&path).unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::Config);
    }
}
