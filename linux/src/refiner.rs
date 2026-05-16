use crate::config::Config;

/// Conservative ASR error corrector that calls an OpenAI-compatible
/// `/chat/completions` endpoint. Direct port of macOS `LLMRefiner.swift`.
///
/// The system prompt is the **product contract** — copied verbatim from
/// `dist/Sources/VoiceInput/LLMRefiner.swift:46-63`. Users have explicitly
/// asked for no rewriting/polishing; preserve "return as-is when in doubt".
pub struct LlmRefiner {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    enabled: bool,
}

/// System prompt — verbatim port from macOS `LLMRefiner.swift:46-63`.
/// Pinned by `system_prompt_is_verbatim` test; edit only with deliberate
/// product intent.
pub(crate) const SYSTEM_PROMPT: &str = "You are a conservative speech recognition error corrector. ONLY fix clear, obvious transcription mistakes. When in doubt, leave the text unchanged.\n\nWhat to fix:\n- English words/acronyms wrongly rendered as Chinese characters (e.g. \"配森\" → \"Python\", \"杰森\" → \"JSON\", \"阿皮爱\" → \"API\")\n- Obvious Chinese homophone errors where context makes the correct character clear\n- Broken English words or phrases split/merged incorrectly by the recognizer\n\nWhat NOT to do:\n- Do NOT rephrase, rewrite, or \"improve\" any text\n- Do NOT add or remove words beyond fixing recognition errors\n- Do NOT change text that could plausibly be correct\n- Do NOT alter punctuation unless clearly wrong\n\nIf the input appears correct, return it exactly as-is. Return ONLY the text, nothing else.";

impl LlmRefiner {
    /// Build a refiner from the loaded `Config`. The reqwest client is
    /// built with a 10 s total timeout — matches macOS `URLRequest.timeoutInterval = 10`.
    pub fn from_config(cfg: &Config) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client build with rustls features");

        Self {
            client,
            base_url: cfg.llm_api_base_url.clone(),
            api_key: cfg.llm_api_key.clone(),
            model: cfg.llm_model.clone(),
            enabled: cfg.llm_enabled,
        }
    }

    /// True when the user has enabled refinement AND provided an API key.
    /// Matches macOS `isConfigured` semantics.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn system_prompt_is_verbatim() {
        // Pin against accidental rephrasing. Edit ONLY with product intent.
        assert!(SYSTEM_PROMPT.contains("conservative speech recognition error corrector"));
        assert!(SYSTEM_PROMPT.contains("ONLY fix clear, obvious transcription mistakes"));
        assert!(SYSTEM_PROMPT.contains("When in doubt, leave the text unchanged"));
        assert!(SYSTEM_PROMPT.contains("\"配森\" → \"Python\""));
        assert!(SYSTEM_PROMPT.contains("\"杰森\" → \"JSON\""));
        assert!(SYSTEM_PROMPT.contains("\"阿皮爱\" → \"API\""));
        assert!(SYSTEM_PROMPT.contains("Do NOT rephrase, rewrite, or \"improve\" any text"));
        assert!(SYSTEM_PROMPT.contains("return it exactly as-is"));
        assert!(SYSTEM_PROMPT.contains("Return ONLY the text"));
    }

    #[test]
    fn from_config_disabled_by_default() {
        let cfg = Config::default();
        let refiner = LlmRefiner::from_config(&cfg);
        assert!(!refiner.is_active(), "default config has llm_enabled=false");
    }

    #[test]
    fn is_active_requires_both_enabled_and_api_key() {
        let mut cfg = Config::default();
        cfg.llm_enabled = true;
        cfg.llm_api_key = String::new();
        assert!(!LlmRefiner::from_config(&cfg).is_active(),
            "enabled but no api key → inactive");

        cfg.llm_api_key = "sk-test".into();
        assert!(LlmRefiner::from_config(&cfg).is_active(),
            "both set → active");

        cfg.llm_enabled = false;
        assert!(!LlmRefiner::from_config(&cfg).is_active(),
            "disabled even with api key → inactive");
    }
}
