use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Master switch. When false, the hotkey is observed but ignored —
    /// no recording, no paste. Mirrors macOS "Enabled" menu item.
    pub enabled: bool,
    pub language_hint: String,
    pub llm_enabled: bool,
    pub llm_api_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub whisper_model_size: String,
    pub whisper_model_path: Option<PathBuf>,
    /// HTTP timeout for the LLM refiner request, in seconds. Default 30 —
    /// generous enough to accommodate a local Ollama cold-start; cloud
    /// providers complete in ~1 s so the longer timeout is invisible.
    pub llm_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            language_hint: "zh".to_string(),
            llm_enabled: false,
            llm_api_base_url: "https://api.openai.com/v1".to_string(),
            llm_api_key: String::new(),
            llm_model: "gpt-4o-mini".to_string(),
            whisper_model_size: "small".to_string(),
            whisper_model_path: None,
            llm_timeout_secs: 30,
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
        assert!(cfg.enabled, "enabled defaults to true");
        assert_eq!(cfg.language_hint, "zh");
        assert_eq!(cfg.llm_api_base_url, "https://api.openai.com/v1");
        assert!(!cfg.llm_enabled);
        assert_eq!(cfg.whisper_model_size, "small");
        assert!(cfg.whisper_model_path.is_none());
        assert_eq!(
            cfg.llm_timeout_secs, 30,
            "30s default accommodates local Ollama cold-start"
        );
    }

    #[test]
    fn missing_timeout_field_falls_back_to_default() {
        // Old config files (pre-Phase 4 timeout field) must continue to load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.toml");
        std::fs::write(
            &path,
            r#"
language_hint = "zh"
llm_enabled = false
llm_api_base_url = "https://api.openai.com/v1"
llm_api_key = ""
llm_model = "gpt-4o-mini"
whisper_model_size = "small"
"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.llm_timeout_secs, 30);
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
