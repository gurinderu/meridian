use std::path::PathBuf;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian_transport::factory::EnvResolver;

fn cfg(id: &str) -> ProfileConfig {
    ProfileConfig { id: id.into(), kind: None, claude_config_dir: None, api_key: None, base_url: None, oauth_token: None }
}

#[test]
fn no_profiles_resolves_default_with_empty_overlay() {
    let s = ProfileStore::new(vec![], PathBuf::from("/cfg"));
    assert_eq!(s.resolve_id(None), "default");
    assert!(s.overlay("default").is_empty());
}

#[test]
fn resolution_priority_header_then_active_then_first() {
    let s = ProfileStore::new(vec![cfg("personal"), cfg("work")], PathBuf::from("/cfg"));
    assert_eq!(s.resolve_id(None), "personal", "falls back to first");
    s.set_active("work".into());
    assert_eq!(s.resolve_id(None), "work", "active wins over first");
    assert_eq!(s.resolve_id(Some("personal")), "personal", "header wins over active");
    assert_eq!(s.resolve_id(Some("ghost")), "personal", "unknown header -> first");
}

#[test]
fn overlay_api_sets_key_and_base_url() {
    let mut p = cfg("api1"); p.kind = Some(ProfileType::Api);
    p.api_key = Some("sk-test".into()); p.base_url = Some("https://api.test".into());
    let s = ProfileStore::new(vec![p], PathBuf::from("/cfg"));
    let o = s.overlay("api1");
    assert_eq!(o.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-test"));
    assert_eq!(o.get("ANTHROPIC_BASE_URL").map(String::as_str), Some("https://api.test"));
    assert!(!o.contains_key("CLAUDE_CONFIG_DIR"));
}

#[test]
fn overlay_oauth_sets_token_and_isolated_config_dir() {
    let mut p = cfg("work"); p.oauth_token = Some("oauth-xyz".into());
    let s = ProfileStore::new(vec![p], PathBuf::from("/cfg"));
    let o = s.overlay("work");
    assert_eq!(o.get("CLAUDE_CODE_OAUTH_TOKEN").map(String::as_str), Some("oauth-xyz"));
    assert_eq!(o.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/cfg/profiles/work"));
}

#[test]
fn overlay_claude_max_overrides_config_dir_when_set() {
    let mut p = cfg("max"); p.claude_config_dir = Some("/home/me/.claude".into());
    let s = ProfileStore::new(vec![p], PathBuf::from("/cfg"));
    assert_eq!(s.overlay("max").get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/home/me/.claude"));
}

#[test]
fn config_deserializes_from_ts_json_shape() {
    let json = r#"[{"id":"work","type":"oauth-token","oauthToken":"t"},
                   {"id":"api1","type":"api","apiKey":"k","baseUrl":"https://b"}]"#;
    let parsed: Vec<ProfileConfig> = serde_json::from_str(json).unwrap();
    assert_eq!(parsed[0].id, "work");
    assert!(matches!(parsed[0].kind, Some(ProfileType::OauthToken)));
    assert_eq!(parsed[1].api_key.as_deref(), Some("k"));
}
