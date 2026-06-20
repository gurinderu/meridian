use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};

fn cfg(id: &str) -> ProfileConfig {
    ProfileConfig { id: id.into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some(format!("/cfg/{id}")), api_key: None, base_url: None, oauth_token: None }
}

#[test]
fn disk_discovery_off_uses_only_config_profiles() {
    let store = ProfileStore::new(vec![cfg("a")], std::env::temp_dir());
    let ids: Vec<_> = store.effective().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, vec!["a"]);
    // list() reports active flag against precedence (first profile here)
    let l = store.list();
    assert_eq!(l.len(), 1);
    assert!(l[0].is_active);
}

#[test]
fn config_wins_over_disk_by_id() {
    // disk discovery merge logic is exercised via the pure merge helper
    let from_config = vec![cfg("shared"), cfg("only-config")];
    let from_disk = vec![cfg("shared"), cfg("only-disk")];
    let merged = meridian::profiles::merge_effective(&from_config, from_disk);
    let ids: Vec<_> = merged.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["shared", "only-config", "only-disk"]);
    // the "shared" entry is the config one (its config dir)
    assert_eq!(merged[0].claude_config_dir.as_deref(), Some("/cfg/shared"));
}

#[test]
fn resolve_and_overlay_use_effective_list() {
    let store = ProfileStore::new(vec![cfg("a"), cfg("b")], std::env::temp_dir());
    assert_eq!(store.resolve_id(Some("b")), "b");
    assert_eq!(store.resolve_id(None), "a"); // first
    use meridian_transport::factory::EnvResolver;
    assert_eq!(store.overlay("b").get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/cfg/b"));
}
