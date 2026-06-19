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
