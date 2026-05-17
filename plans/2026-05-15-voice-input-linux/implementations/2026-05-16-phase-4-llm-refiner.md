# Phase 4: LLM Refiner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port macOS `LLMRefiner.swift` to Linux as `refiner.rs` — an OpenAI-compatible HTTP client that conservatively fixes ASR errors (e.g. `配森 → Python`) before the transcript is pasted, with a fail-safe fallback to raw text when the API is unavailable.

**Architecture:** Single async `LlmRefiner` struct built from `Config`. Holds a `reqwest::Client` (with 10 s timeout) plus the configured base URL / API key / model / enabled flag. Two entrypoints: `refine(text, force) -> String` (fail-safe — paste flow) and `try_refine(text, force) -> AppResult<String>` (error-surfacing — Settings → Test in Phase 5). The conservative system prompt is copied verbatim from `dist/Sources/VoiceInput/LLMRefiner.swift:46-63` and pinned against drift by a unit test. Integration: instantiate once in `run_listen`, call `refine()` between the pipeline drain and the ydotool paste.

**Tech Stack:** reqwest 0.12 (rustls-tls), serde_json 1, wiremock 0.6 (dev-only).

---

## Pre-flight: Phase 3 entry conditions

Before starting, verify these from Phase 3:

- `main` at `e49d4a3` (clippy fix) — confirm with `git log --oneline -1`
- 35 unit + integration tests pass: `cd linux && cargo test 2>&1 | grep "test result"`
- `Config` has `llm_enabled`, `llm_api_base_url`, `llm_api_key`, `llm_model` fields (verified)
- `AppError::NetworkError(String)` already exists at `linux/src/error.rs:33` — no new variant needed

Branch: `linux/phase-4-llm-refiner` (create from main).

```bash
cd /home/desmond/Repos/voice-input-src
git checkout main
git pull --ff-only
git checkout -b linux/phase-4-llm-refiner
```

---

## File structure

```
linux/
├── Cargo.toml                   # add reqwest, serde_json; wiremock dev-only
├── src/
│   ├── refiner.rs               # NEW — LlmRefiner struct, system prompt, refine/try_refine
│   ├── lib.rs                   # add `pub mod refiner;`
│   └── main.rs                  # wire LlmRefiner into run_listen
└── tests/
    └── refiner_http.rs          # NEW integration test with wiremock
```

---

## Task 4.1: Add HTTP client dependencies

**Files:**
- Modify: `linux/Cargo.toml`

The Linux refiner needs:
- `reqwest` 0.12 with `rustls-tls` (no OpenSSL dependency — better for static distribution) and `json` feature.
- `serde_json` 1 for response parsing (the request body uses `serde_json::json!` macro).
- `wiremock` 0.6 dev-dep for HTTP mocking in unit tests.

- [ ] **Step 1: Update `linux/Cargo.toml`**

Find the existing `[dependencies]` block (lines 10–31). Add after the existing alphabetical entries (in alphabetical position):

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
serde_json = "1"
```

Insert in alphabetical order — `reqwest` after `rubato`, `serde_json` after `serde`.

Find `[dev-dependencies]`:

```toml
[dev-dependencies]
tempfile = "3"
```

Replace with:

```toml
[dev-dependencies]
tempfile = "3"
wiremock = "0.6"
```

- [ ] **Step 2: Build to verify the deps resolve**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
```

Expected: clean build (no errors, may take 60–120 s on first compile to pull rustls).

If you see `error: failed to select a version for ...` related to OpenSSL/native-tls, double-check that `default-features = false` is set on `reqwest` — that disables the default `native-tls` backend.

- [ ] **Step 3: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml linux/Cargo.lock
git commit -m "feat(linux): add reqwest + wiremock for LLM refiner"
```

---

## Task 4.2: Refiner skeleton — struct, constructor, system prompt pin

**Files:**
- Create: `linux/src/refiner.rs`
- Modify: `linux/src/lib.rs`

This task creates the module shell. No HTTP yet — just struct, `from_config` constructor, the system prompt constant, and ONE pin test to lock the prompt against accidental edits.

The prompt is the product contract. Copying it verbatim is mandatory: the user explicitly requested conservative behavior. See `dist/Sources/VoiceInput/LLMRefiner.swift:46-63`.

- [ ] **Step 1: Create `linux/src/refiner.rs`**

```rust
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
```

- [ ] **Step 2: Register the module in `linux/src/lib.rs`**

Find:

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod injector;
pub mod overlay;
pub mod speech;
pub mod tray;
```

Insert `pub mod refiner;` in alphabetical order (between `overlay` and `speech`):

```rust
pub mod app;
pub mod audio;
pub mod cli;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod injector;
pub mod overlay;
pub mod refiner;
pub mod speech;
pub mod tray;
```

- [ ] **Step 3: Run the new tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --lib refiner 2>&1 | tail -10
```

Expected:
```
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

If the prompt-pin test fails, you've drifted the prompt — fix the constant, not the test.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/refiner.rs linux/src/lib.rs
git commit -m "feat(linux): add LlmRefiner skeleton with verbatim macOS system prompt"
```

---

## Task 4.3: `try_refine` happy path

**Files:**
- Modify: `linux/src/refiner.rs`
- Create: `linux/tests/refiner_http.rs`

Implement the actual HTTP call. `try_refine` returns `AppResult<String>` — used directly by the future Settings → Test flow (Phase 5). The fail-safe wrapper (`refine`) comes in Task 4.4.

Request shape (must match OpenAI / OpenAI-compatible APIs):
```json
POST /chat/completions
Authorization: Bearer <api_key>
Content-Type: application/json

{
  "model": "<model>",
  "messages": [
    {"role": "system", "content": SYSTEM_PROMPT},
    {"role": "user", "content": "<text>"}
  ],
  "temperature": 0.3
}
```

Response parse: `json["choices"][0]["message"]["content"]` → trim whitespace.

- [ ] **Step 1: Add `try_refine` to `linux/src/refiner.rs`**

Find the existing `impl LlmRefiner { ... }` block (after the `is_active` method). Insert these methods inside the `impl` block, before the closing `}`:

```rust
    /// Call the LLM and return refined text. Errors propagate — callers
    /// that want fail-safe behavior should use `refine` instead.
    ///
    /// When `force` is false and the refiner is inactive (disabled OR
    /// no api_key configured), returns the input unchanged WITHOUT
    /// making a network call. Force-bypass exists so Phase 5's
    /// Settings → Test button can verify configuration before saving.
    pub async fn try_refine(&self, text: &str, force: bool) -> crate::error::AppResult<String> {
        if !force && !self.is_active() {
            return Ok(text.to_string());
        }
        if text.is_empty() {
            return Ok(String::new());
        }

        // Trim a trailing slash on the base URL so we don't double up.
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/chat/completions", base);

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": text},
            ],
            "temperature": 0.3,
        });

        tracing::info!(url = %url, model = %self.model, "llm refine request");

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::error::AppError::NetworkError(format!("llm refine send: {}", e))
            })?;

        let status = resp.status();
        let raw = resp.text().await.map_err(|e| {
            crate::error::AppError::NetworkError(format!("llm refine body: {}", e))
        })?;

        if !status.is_success() {
            return Err(crate::error::AppError::NetworkError(format!(
                "llm refine non-2xx: {} body={}",
                status, raw
            )));
        }

        let json: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            crate::error::AppError::NetworkError(format!(
                "llm refine parse: {} body={}",
                e, raw
            ))
        })?;

        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                crate::error::AppError::NetworkError(format!(
                    "llm refine missing choices[0].message.content; body={}",
                    raw
                ))
            })?;

        let trimmed = content.trim().to_string();
        tracing::info!(
            input_bytes = text.len(),
            output_bytes = trimmed.len(),
            "llm refine response"
        );
        Ok(trimmed)
    }
```

- [ ] **Step 2: Add an internal constructor for tests that points at a custom URL**

Add this method inside the same `impl LlmRefiner` block, right after `try_refine`:

```rust
    /// Test-only constructor that points at a wiremock server. Public
    /// (not `cfg(test)`-gated) because `tests/` integration tests live in
    /// a separate crate and can't see `#[cfg(test)]` items.
    #[doc(hidden)]
    pub fn for_test(base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            enabled: true,
        }
    }
```

- [ ] **Step 3: Create `linux/tests/refiner_http.rs`** with three happy-path tests

```rust
use voice_input::refiner::LlmRefiner;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn happy_path_returns_refined_text() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_partial_json(serde_json::json!({
            "model": "gpt-4o-mini",
            "temperature": 0.3,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [
                {"message": {"role": "assistant", "content": "Python and JSON"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("配森 和 杰森", false).await.unwrap();
    assert_eq!(out, "Python and JSON");
}

#[tokio::test]
async fn trailing_slash_in_base_url_does_not_double_up() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let url_with_slash = format!("{}/", server.uri());
    let refiner = LlmRefiner::for_test(url_with_slash, "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("hi", false).await.unwrap();
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn response_content_is_whitespace_trimmed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "  Python and JSON  \n"}}]
        })))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("配森", false).await.unwrap();
    assert_eq!(out, "Python and JSON");
}
```

- [ ] **Step 4: Run the integration tests**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test --test refiner_http 2>&1 | tail -10
```

Expected:
```
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Common failures and fixes:
- `wiremock request did not match` → the request body / headers diverged from what's mounted. Mount with `.respond_with(ResponseTemplate::new(200))` once without matchers to see what landed.
- `tokio::main` not found → tokio with `macros` + `rt-multi-thread` features is required; both are already in `Cargo.toml`.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/refiner.rs linux/tests/refiner_http.rs
git commit -m "feat(linux): implement LlmRefiner::try_refine with wiremock happy-path tests"
```

---

## Task 4.4: Fail-safe `refine` + error tests

**Files:**
- Modify: `linux/src/refiner.rs`
- Modify: `linux/tests/refiner_http.rs`

`run_listen` must NEVER fail paste because the LLM is down. Wrap `try_refine` in a fail-safe `refine` that logs the error and returns the raw text. This separation lets Phase 5's Settings → Test surface errors explicitly.

- [ ] **Step 1: Add `refine` to `linux/src/refiner.rs`**

Inside the same `impl LlmRefiner` block, right after `try_refine` (before `for_test`), insert:

```rust
    /// Fail-safe wrapper around `try_refine` — network/parse errors are
    /// logged at `warn` and the original text is returned. Use this from
    /// the listen flow where paste must succeed even when the LLM is
    /// unreachable.
    pub async fn refine(&self, text: &str, force: bool) -> String {
        match self.try_refine(text, force).await {
            Ok(refined) => refined,
            Err(e) => {
                tracing::warn!(error = %e, "llm refine failed; falling back to raw text");
                text.to_string()
            }
        }
    }
```

- [ ] **Step 2: Add fallback tests to `linux/tests/refiner_http.rs`**

Append these tests to the existing `linux/tests/refiner_http.rs`:

```rust
#[tokio::test]
async fn disabled_refiner_short_circuits_without_request() {
    // No mocks mounted — any HTTP call would 404 and fail the assertion below.
    let server = MockServer::start().await;
    // Empty api_key marks the refiner inactive (matches Config default).
    let refiner = LlmRefiner::for_test(server.uri(), "", "gpt-4o-mini");

    let out = refiner.try_refine("hello", false).await.unwrap();
    assert_eq!(out, "hello", "inactive refiner must not contact the server");
}

#[tokio::test]
async fn force_bypasses_inactive_guard() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "forced"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Empty api_key → normally inactive, but force=true bypasses
    let refiner = LlmRefiner::for_test(server.uri(), "", "gpt-4o-mini");
    let out = refiner.try_refine("hi", true).await.unwrap();
    assert_eq!(out, "forced");
}

#[tokio::test]
async fn network_5xx_yields_error_from_try_refine() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream down"))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let err = refiner.try_refine("hello", false).await.unwrap_err();
    assert!(err.to_string().contains("non-2xx"), "expected non-2xx error, got: {}", err);
}

#[tokio::test]
async fn refine_falls_back_to_raw_text_on_5xx() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.refine("hello world", false).await;
    assert_eq!(out, "hello world", "refine must fall back to raw text on API errors");
}

#[tokio::test]
async fn refine_falls_back_when_response_missing_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": []  // missing choices[0].message.content
        })))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.refine("hello", false).await;
    assert_eq!(out, "hello", "malformed response must fall back to raw text");
}
```

- [ ] **Step 3: Run all refiner tests + the full suite**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected counts:
- `lib` tests: 28 passed (25 prior + 3 new refiner unit tests)
- `tests/refiner_http.rs`: 8 passed (3 happy + 5 fail-safe)
- Plus existing integration tests (config_roundtrip 2, resolve_model_path 3, vad_slicing 3, audio_rms 2)

Total: ≥40 passed, 0 failed, 2 ignored.

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/refiner.rs linux/tests/refiner_http.rs
git commit -m "feat(linux): add fail-safe LlmRefiner::refine with 5xx + malformed-response tests"
```

---

## Task 4.5: Wire refiner into `run_listen_async`

**Files:**
- Modify: `linux/src/main.rs`

Construct one `LlmRefiner` at the top of `run_listen_async` and call `refine()` after segments join, before `inject_text`. The refiner reuses its `reqwest::Client` across record cycles (the client owns connection pool state — important for latency on repeat calls).

- [ ] **Step 1: Construct the refiner before the loop**

In `linux/src/main.rs`, locate `run_listen_async`. Find this block:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();
```

Insert refiner construction right after `language_hint`:

```rust
    let mut current_pipeline: Option<speech::PipelineHandle> = None;
    let mut current_capture: Option<voice_input::audio::Capture> = None;
    let language_hint = cfg.language_hint.clone();
    let refiner = voice_input::refiner::LlmRefiner::from_config(&cfg);
    tracing::info!(active = refiner.is_active(), "llm refiner initialized");
```

- [ ] **Step 2: Move the "Refining…" overlay text into the non-empty branch**

The current code sends `OverlayCmd::SetText("Refining…")` at the top of the deactivated arm — even when there's nothing to refine. That looked weird visually. Move it to where it's accurate.

Find this block at the top of the deactivated arm:

```rust
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    let _ = overlay_tx.send(OverlayCmd::SetText("Refining…".into()));
                    drop(current_capture.take());
```

Delete the `let _ = overlay_tx.send(...)` line:

```rust
            Some(_deactivated) = deactivated.next() => {
                if let Some(pipeline) = current_pipeline.take() {
                    tracing::info!("shortcut released; draining and pasting");
                    drop(current_capture.take());
```

- [ ] **Step 3: Replace the drain → paste block with a drain → refine → paste block**

Find this block:

```rust
                    let segments = tokio::task::spawn_blocking(move || pipeline.join_remaining())
                        .await
                        .context("draining pipeline")?;
                    let joined = segments.join(" ").trim().to_string();
                    if joined.is_empty() {
                        tracing::info!("no segments transcribed; skipping paste");
                    } else {
                        tracing::info!(segments = segments.len(), bytes = joined.len(), "pasting");
                        let injected = tokio::task::spawn_blocking({
                            let joined = joined.clone();
                            move || voice_input::injector::inject_text(&joined)
                        })
                        .await
                        .context("ydotool paste task")?;
                        if let Err(e) = injected {
                            tracing::error!(error = %e, "paste failed");
                        }
                    }
```

Replace with:

```rust
                    let segments = tokio::task::spawn_blocking(move || pipeline.join_remaining())
                        .await
                        .context("draining pipeline")?;
                    let raw_joined = segments.join(" ").trim().to_string();
                    if raw_joined.is_empty() {
                        tracing::info!("no segments transcribed; skipping paste");
                    } else {
                        // Refine before paste. The refiner short-circuits when
                        // disabled/unconfigured; on errors it logs and returns the
                        // raw text — paste must not fail because the LLM is down.
                        let _ = overlay_tx.send(OverlayCmd::SetText("Refining…".into()));
                        let to_paste = refiner.refine(&raw_joined, false).await;
                        tracing::info!(
                            segments = segments.len(),
                            raw_bytes = raw_joined.len(),
                            final_bytes = to_paste.len(),
                            "pasting"
                        );
                        let injected = tokio::task::spawn_blocking({
                            let to_paste = to_paste.clone();
                            move || voice_input::injector::inject_text(&to_paste)
                        })
                        .await
                        .context("ydotool paste task")?;
                        if let Err(e) = injected {
                            tracing::error!(error = %e, "paste failed");
                        }
                    }
```

- [ ] **Step 4: Build + test**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: clean build; all tests pass (same counts as Task 4.4).

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/src/main.rs
git commit -m "feat(linux): call LlmRefiner between pipeline drain and ydotool paste"
```

---

## Task 4.6: README + manual smoke test

**Files:**
- Modify: `linux/README.md`

Document Phase 4 status and the LLM config keys. Then user runs a real OpenAI-compatible test.

- [ ] **Step 1: Update the Status block at the top of `linux/README.md`**

Find:

```markdown
> Status: **Phase 3** — overlay capsule with live waveform appears during dictation. Tray (default), transcribe CLI (Phase 1), and listen mode (Phase 2) all still work.
```

Replace with:

```markdown
> Status: **Phase 4** — optional LLM refinement of transcripts before paste (OpenAI-compatible APIs). Overlay (Phase 3), tray, transcribe CLI, and listen mode all still work.
```

- [ ] **Step 2: Add an LLM section before the "Compositor support" heading**

After the existing `### Overlay capsule (Phase 3)` block, before the `## Compositor support` heading, insert:

```markdown
### LLM refinement (Phase 4)

Optionally pass the raw transcript through an OpenAI-compatible chat completion before pasting. The system prompt is intentionally conservative — it fixes ASR errors (`配森 → Python`, `杰森 → JSON`, etc.) but does NOT rewrite or polish the text. If the API is unreachable, paste falls back to the raw transcript.

Configure via `~/.config/voice-input/config.toml`:

~~~toml
llm_enabled = true
llm_api_base_url = "https://api.openai.com/v1"
llm_api_key = "sk-..."
llm_model = "gpt-4o-mini"
~~~

The `llm_api_base_url` accepts any OpenAI-compatible endpoint (vLLM, llama.cpp server, Together, Groq, etc.). The 10 s request timeout matches the macOS app. A future Settings UI (Phase 5) will replace manual TOML editing.
```

- [ ] **Step 3: Verify the README change**

```bash
grep -c "Phase 4" /home/desmond/Repos/voice-input-src/linux/README.md
```

Expected: ≥2 mentions of "Phase 4".

- [ ] **Step 4: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/README.md
git commit -m "docs(linux): document LLM refiner config and Phase 4 status"
```

- [ ] **Step 5: User-driven manual smoke test**

USER hands this back. To run:

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo build --release
```

Edit `~/.config/voice-input/config.toml`:
- Set `llm_enabled = true`
- Set `llm_api_key = "<your sk-... key>"`
- Keep `llm_api_base_url = "https://api.openai.com/v1"` and `llm_model = "gpt-4o-mini"` (or swap to any OpenAI-compatible endpoint)

Then:

```bash
RUST_LOG=info ./target/release/voice-input listen
```

1. Hold the configured hotkey.
2. Speak a phrase whisper mis-renders, e.g. **"Elon Musk 在 California 的 Tesla 总部宣布了新计划"** (Phase 2 transcribed this as "Pino Mask 在 California 的 Tesla 总部宣布了新计划" — exactly the kind of ASR error the refiner targets).
3. Release. The capsule briefly shows "Refining…", then paste happens.
4. Expected: "Elon Musk" appears in the focused text app, not "Pino Mask".
5. Check log: terminal where `voice-input listen` runs should show `llm refine request` / `llm refine response` lines.

Acceptance:
- ✅ With `llm_enabled = true` and a valid API key: refined text pastes (corrects ASR errors).
- ✅ With `llm_enabled = false`: behavior identical to Phase 3 (raw whisper transcript pastes).
- ✅ With `llm_enabled = true` but invalid key / wrong base URL: paste still happens with raw transcript; warn log shows the network error.

If the LLM call fails silently and you can't tell whether it refined, run with `RUST_LOG=info,voice_input=debug` and watch for `llm refine response` lines. If those never appear when `llm_enabled = true`, check `llm_api_key` is non-empty and `active=true` in the startup log.

Report findings.

---

## Task 4.7: Final verification + push

- [ ] **Step 1: Full test run**

```bash
cd /home/desmond/Repos/voice-input-src/linux
PATH="$HOME/.cargo/bin:$PATH" cargo test 2>&1 | grep "test result"
```

Expected: at least 40 passed (28 lib + 8 refiner_http + 4 prior integration), 0 failed, 2 ignored.

- [ ] **Step 2: Release build, no warnings**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo build --release 2>&1 | grep -E "warning|error" | grep -v Compiling | head -20
```

Expected: empty (no voice-input warnings).

- [ ] **Step 3: cargo fmt + clippy**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo fmt --check 2>&1
# If non-empty:
PATH="$HOME/.cargo/bin:$PATH" cargo fmt
git -C /home/desmond/Repos/voice-input-src add -u linux/
git -C /home/desmond/Repos/voice-input-src commit -m "style(linux): cargo fmt"

PATH="$HOME/.cargo/bin:$PATH" cargo clippy --all-targets -- -D warnings 2>&1 | tail -30
# If clippy finds issues, fix trivial ones; STOP and report non-trivial ones.
# If fixes applied:
git -C /home/desmond/Repos/voice-input-src add linux/
git -C /home/desmond/Repos/voice-input-src commit -m "chore(linux): fix clippy findings"
```

- [ ] **Step 4: Push the branch**

```bash
cd /home/desmond/Repos/voice-input-src
git push -u origin linux/phase-4-llm-refiner 2>&1
```

- [ ] **Step 5: Final verification**

```bash
git -C /home/desmond/Repos/voice-input-src log origin/main..HEAD --oneline
git -C /home/desmond/Repos/voice-input-src status
git -C /home/desmond/Repos/voice-input-src branch -vv
```

Expected: clean tree, branch tracking `origin/linux/phase-4-llm-refiner`, ~7 commits ahead of main.

---

## Self-Review Notes

**Spec coverage** (from `plans/voice-input-linux.md` Phase 4):
- ✅ Direct port of `LLMRefiner.swift` → Task 4.2 (struct, constructor) + Task 4.3 (HTTP)
- ✅ OpenAI-compatible `/chat/completions` → Task 4.3
- ✅ Conservative system prompt **verbatim** → Task 4.2 with `system_prompt_is_verbatim` pin test
- ✅ `force` flag for Settings → Test → Task 4.3 `try_refine(text, force)` + Task 4.4 `force_bypasses_inactive_guard`
- ✅ Persistence keys `llm_enabled / llm_api_base_url / llm_api_key / llm_model` — already in `Config` since Phase 0; refiner consumes them in Task 4.2 `from_config`
- ✅ Logging via `tracing` (Linux equivalent of macOS `os.Logger`) → Task 4.3 emits info-level request/response logs
- ✅ 10 s timeout → Task 4.2 `reqwest::Client::builder().timeout(10s)`
- ✅ Temperature 0.3 → Task 4.3 request body
- ✅ Fail-safe paste on LLM error → Task 4.4 `refine` wrapper + tests
- ⏸ `cancel()` (macOS calls it when a new recording starts before the previous refine completes) — deferred. In Linux `run_listen_async` the hotkey loop awaits the refine + paste before the next activated event is processed, so the race doesn't apply in MVP. Add cancellation if user-reported as a problem.

**Phase 3 entry conditions addressed:** Phase 3 merged + pushed; `main` at `e49d4a3`; all tests green at branch creation.

**Placeholder scan:** Searched for "TBD", "TODO", "implement later", "Add appropriate", "handle edge cases", "Write tests for the above", "Similar to Task". None present in plan body.

**Type consistency check:**
- `LlmRefiner::from_config(&Config) -> Self` consistent across Tasks 4.2, 4.5.
- `LlmRefiner::is_active(&self) -> bool` consistent across Tasks 4.2, 4.5.
- `LlmRefiner::try_refine(&self, &str, bool) -> AppResult<String>` consistent across Tasks 4.3, 4.4.
- `LlmRefiner::refine(&self, &str, bool) -> String` consistent across Tasks 4.4, 4.5.
- `LlmRefiner::for_test(impl Into<String> × 3) -> Self` defined Task 4.3, used Tasks 4.3 + 4.4.
- `SYSTEM_PROMPT: &str` (pub(crate) const) defined Task 4.2, referenced Task 4.3 — same module so the visibility holds.
- `AppError::NetworkError(String)` — already exists in `error.rs:33`; no new variant needed.

**Known risk:**
- `reqwest` first compile pulls many crates (rustls-tls, h2, hyper). Expect 2–5 min on a clean target/. Subsequent compiles use the cache.
