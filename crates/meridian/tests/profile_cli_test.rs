use meridian::profile_cli::{is_valid_profile_id, load_profiles_json_at,
    add_oauth_token, dirs_to_remove_on_remove};
use meridian::profiles::{ProfileConfig, ProfileType};

#[test]
fn id_validation() {
    assert!(is_valid_profile_id("work-1_x"));
    assert!(!is_valid_profile_id(""));
    assert!(!is_valid_profile_id("has space"));
    assert!(!is_valid_profile_id("dots.bad"));
}

#[test]
fn add_oauth_token_persists_and_rejects_dupes() {
    let dir = std::env::temp_dir().join(format!("mer-pcli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profiles.json");
    add_oauth_token(&path, "ci", "sk-ant-oat-xxx").unwrap();
    let loaded = load_profiles_json_at(&path);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "ci");
    assert_eq!(loaded[0].oauth_token.as_deref(), Some("sk-ant-oat-xxx"));
    // duplicate rejected
    assert!(add_oauth_token(&path, "ci", "other").is_err());
    // invalid id rejected
    assert!(add_oauth_token(&path, "bad id", "t").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_dirs_for_oauth_and_browser_profiles() {
    let pdir = std::path::Path::new("/root/profiles");
    // oauth-token: isolation dir profiles/<id>
    let oauth = ProfileConfig { id: "ci".into(), kind: Some(ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None, oauth_token: Some("t".into()) };
    assert_eq!(dirs_to_remove_on_remove(&oauth, pdir), vec![pdir.join("ci")]);
    // browser profile with config dir under profiles_dir
    let browser = ProfileConfig { id: "work".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some("/root/profiles/work".into()), api_key: None, base_url: None, oauth_token: None };
    assert_eq!(dirs_to_remove_on_remove(&browser, pdir), vec![std::path::PathBuf::from("/root/profiles/work")]);
    // config dir OUTSIDE profiles_dir is not removed
    let imported = ProfileConfig { id: "home".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some("/home/u/.claude".into()), api_key: None, base_url: None, oauth_token: None };
    assert!(dirs_to_remove_on_remove(&imported, pdir).is_empty());
}
