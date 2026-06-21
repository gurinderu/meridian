use std::collections::HashMap;
use std::path::PathBuf;
use meridian_transport::spawn::{build_args, build_env, SpawnConfig};

fn cfg() -> SpawnConfig {
    SpawnConfig {
        config_dir: PathBuf::from("/tmp/iso"),
        model: Some("claude-opus-4-8".into()),
        mcp_config: Some(serde_json::json!({"mcpServers":{}})),
        include_partial_messages: true,
        resume: None,
        max_turns: None,
        env_overlay: Default::default(),
    }
}

#[test]
fn args_contain_confirmed_stream_json_flags() {
    let a = build_args(&cfg());
    for f in ["--output-format","stream-json","--input-format","--verbose","--include-partial-messages"] {
        assert!(a.iter().any(|x| x == f), "missing {f} in {a:?}");
    }
    assert!(a.windows(2).any(|w| w[0]=="--model" && w[1]=="claude-opus-4-8"));
}

#[test]
fn env_isolates_config_dir_and_strips_secrets() {
    let mut base = HashMap::new();
    base.insert("ANTHROPIC_API_KEY".into(), "sk-should-be-stripped".into());
    base.insert("NODE_OPTIONS".into(), "--max-old-space-size=99".into());
    base.insert("PATH".into(), "/usr/bin".into());
    let env = build_env(&cfg(), &base);
    assert_eq!(env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/tmp/iso"));
    assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    assert!(!env.contains_key("NODE_OPTIONS"));
    assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
    // Realigns the keychain key so the default OAuth token is found under an
    // isolated config dir -> auth succeeds -> streaming partials flow.
    assert_eq!(env.get("CLAUDE_SECURESTORAGE_CONFIG_DIR").map(String::as_str), Some(""));
}

#[test]
fn args_include_resume_when_set() {
    let mut c = cfg();
    c.resume = Some("sess-xyz".into());
    let a = build_args(&c);
    assert!(a.windows(2).any(|w| w[0] == "--resume" && w[1] == "sess-xyz"), "missing --resume in {a:?}");
    // and absent when None
    let a2 = build_args(&cfg());
    assert!(!a2.iter().any(|x| x == "--resume"), "--resume must be absent when None");
}

#[test]
fn args_include_max_turns_when_set() {
    let mut c = cfg();
    c.max_turns = Some(3);
    assert!(build_args(&c).windows(2).any(|w| w[0]=="--max-turns" && w[1]=="3"));
}

#[test]
fn env_overlay_is_applied_after_strip_and_wins() {
    let mut base = HashMap::new();
    base.insert("ANTHROPIC_API_KEY".to_string(), "host-key".to_string());
    let mut c = cfg();
    // api-profile overlay: must survive the strip of ANTHROPIC_API_KEY and override the base config dir.
    c.env_overlay = HashMap::from([
        ("ANTHROPIC_API_KEY".to_string(), "profile-key".to_string()),
        ("ANTHROPIC_BASE_URL".to_string(), "https://example.test".to_string()),
        ("CLAUDE_CONFIG_DIR".to_string(), "/overridden".to_string()),
    ]);
    let env = build_env(&c, &base);
    assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("profile-key"), "overlay survives strip + wins");
    assert_eq!(env.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("https://example.test"));
    assert_eq!(env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/overridden"), "overlay overrides base config dir");
}

#[test]
fn overlay_auth_skips_keychain_realignment() {
    // An api profile's config dir must stay ISOLATED from the host keychain: the
    // keychain must NOT be realigned to the host default OAuth token, so only the
    // profile's own ANTHROPIC_API_KEY is in play.
    let mut c = cfg();
    c.env_overlay = HashMap::from([("ANTHROPIC_API_KEY".to_string(), "sk".to_string())]);
    assert!(!build_env(&c, &HashMap::new()).contains_key("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        "api-profile auth must not realign the keychain to the host default OAuth token");
    // Same for an oauth-token profile: its token governs, not host creds.
    let mut c2 = cfg();
    c2.env_overlay = HashMap::from([("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "t".to_string())]);
    assert!(!build_env(&c2, &HashMap::new()).contains_key("CLAUDE_SECURESTORAGE_CONFIG_DIR"));
}

#[test]
fn empty_overlay_preserves_current_build_env() {
    let mut base = HashMap::new();
    base.insert("ANTHROPIC_API_KEY".to_string(), "host-key".to_string());
    let env = build_env(&cfg(), &base); // cfg() now has an empty overlay
    assert!(!env.contains_key("ANTHROPIC_API_KEY"), "still stripped when no overlay");
    assert!(env.contains_key("CLAUDE_CONFIG_DIR"));
    assert_eq!(env.get("CLAUDE_SECURESTORAGE_CONFIG_DIR").map(String::as_str), Some(""));
}

#[test]
fn lean_defaults_are_set_and_overridable() {
    let env = build_env(&cfg(), &HashMap::new());
    for k in ["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "DISABLE_NON_ESSENTIAL_MODEL_CALLS",
              "DISABLE_AUTOUPDATER", "DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING"] {
        assert_eq!(env.get(k).map(String::as_str), Some("1"), "{k} should be forced on");
    }
    // a host value for one of these is overridden (forced on)
    let mut base = HashMap::new();
    base.insert("DISABLE_AUTOUPDATER".to_string(), "0".to_string());
    assert_eq!(build_env(&cfg(), &base).get("DISABLE_AUTOUPDATER").map(String::as_str), Some("1"));
    // but a profile overlay still wins last
    let mut c = cfg();
    c.env_overlay = HashMap::from([("DISABLE_TELEMETRY".to_string(), "0".to_string())]);
    assert_eq!(build_env(&c, &HashMap::new()).get("DISABLE_TELEMETRY").map(String::as_str), Some("0"));
}
