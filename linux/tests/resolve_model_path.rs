use std::path::PathBuf;

use voice_input::config::Config;

/// Set $VOICE_INPUT_MODEL_PATH for the duration of the test.
/// SAFETY: tests in this file run with parallel cargo test; we make each
/// test idempotent by always setting the env var explicitly (no test
/// relies on the env var being unset). The set/restore is racy with
/// other tests that read this var; acceptable for this isolated test
/// file.
fn with_env_var<R>(value: &str, f: impl FnOnce() -> R) -> R {
    let key = "VOICE_INPUT_MODEL_PATH";
    let prev = std::env::var(key).ok();
    // SAFETY: set_var is `unsafe` in Rust 1.83+ due to cross-thread race
    // risk; we accept the risk for these isolated config tests.
    unsafe {
        std::env::set_var(key, value);
    }
    let r = f();
    unsafe {
        match prev {
            Some(p) => std::env::set_var(key, p),
            None => std::env::remove_var(key),
        }
    }
    r
}

#[test]
fn env_var_override_wins() {
    let cfg = Config::default();
    with_env_var("/tmp/voice-input-test-env.bin", || {
        let path = cfg.resolve_model_path().expect("resolve");
        assert_eq!(path, PathBuf::from("/tmp/voice-input-test-env.bin"));
    });
}

#[test]
fn config_field_wins_over_default() {
    let mut cfg = Config::default();
    cfg.whisper_model_path = Some(PathBuf::from("/tmp/voice-input-test-config.bin"));
    with_env_var("", || {
        let path = cfg.resolve_model_path().expect("resolve");
        assert_eq!(path, PathBuf::from("/tmp/voice-input-test-config.bin"));
    });
}

#[test]
fn default_uses_xdg_data_dir() {
    let cfg = Config::default();
    with_env_var("", || {
        let path = cfg.resolve_model_path().expect("resolve");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with("voice-input/models/ggml-small.bin"),
            "expected XDG path ending with voice-input/models/ggml-small.bin, got {}",
            path_str
        );
    });
}
