use std::collections::HashMap;
use std::path::PathBuf;
use meridian_transport::spawn::{build_args, build_env, SpawnConfig};

fn cfg() -> SpawnConfig {
    SpawnConfig {
        config_dir: PathBuf::from("/tmp/iso"),
        model: Some("claude-opus-4-8".into()),
        mcp_config: Some(serde_json::json!({"mcpServers":{}})),
        include_partial_messages: true,
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
}
