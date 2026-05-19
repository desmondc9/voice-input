use voice_input::config::Config;

#[test]
fn write_then_read_yields_same_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    let original = Config {
        language_hint: "ja".into(),
        llm_enabled: true,
        llm_api_key: "sk-test-12345".into(),
        ..Default::default()
    };

    original.save_to(&path).expect("save");
    let loaded = Config::load_from(&path).expect("load");

    assert_eq!(loaded, original);
}

#[test]
fn save_creates_parent_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("a").join("b").join("c").join("config.toml");

    Config::default()
        .save_to(&nested)
        .expect("save creates parents");
    assert!(nested.exists());
}
