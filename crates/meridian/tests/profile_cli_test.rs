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

#[test]
fn add_oauth_token_rejects_empty_or_whitespace_token() {
    let dir = std::env::temp_dir().join(format!("mer-pcli-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profiles.json");
    assert!(add_oauth_token(&path, "ci", "").is_err());
    assert!(add_oauth_token(&path, "ci2", "   ").is_err());
    // nothing should have been written
    assert!(load_profiles_json_at(&path).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn profiles_json_is_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("mer-pcli-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profiles.json");
    add_oauth_token(&path, "ci", "sk-ant-oat-xxx").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "profiles.json must be private (0o600), got {mode:o}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_dirs_rejects_parent_dir_traversal() {
    let pdir = std::path::Path::new("/root/profiles");
    let evil = ProfileConfig { id: "x".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some("/root/profiles/../../etc".into()),
        api_key: None, base_url: None, oauth_token: None };
    assert!(dirs_to_remove_on_remove(&evil, pdir).is_empty(),
        "a claudeConfigDir containing .. must not be scheduled for deletion");
}

#[test]
fn remove_dirs_sanitizes_oauth_token_id_traversal() {
    // an oauth-token profile whose id contains traversal must NOT escape
    // profiles_dir — the isolation dir is sanitized to a single segment.
    let pdir = std::path::Path::new("/root/profiles");
    let evil = ProfileConfig { id: "../../etc".into(), kind: Some(ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None, oauth_token: Some("t".into()) };
    let dirs = dirs_to_remove_on_remove(&evil, pdir);
    for d in &dirs {
        assert!(d.starts_with(pdir), "{d:?} must stay under {pdir:?}");
        assert!(!d.to_string_lossy().contains(".."), "no .. in {d:?}");
    }
}
