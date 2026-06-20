use meridian::settings::{load_settings_at, save_settings_at, MeridianSettings};

#[test]
fn save_then_load_roundtrips_active_profile() {
    let dir = std::env::temp_dir().join(format!("mer-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");

    save_settings_at(&path, MeridianSettings { active_profile: Some("work".into()) }).unwrap();
    let s = load_settings_at(&path);
    assert_eq!(s.active_profile.as_deref(), Some("work"));

    // missing file -> empty
    let _ = std::fs::remove_file(&path);
    assert_eq!(load_settings_at(&path).active_profile, None);
    // invalid json -> empty (not a panic)
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(load_settings_at(&path).active_profile, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_merges_and_does_not_clobber_unknown_keys() {
    let dir = std::env::temp_dir().join(format!("mer-settings-merge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    // a pre-existing file with an unknown key
    std::fs::write(&path, b"{\n  \"theme\": \"dark\"\n}\n").unwrap();
    save_settings_at(&path, MeridianSettings { active_profile: Some("p1".into()) }).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"theme\""), "unknown key must survive merge");
    assert!(raw.contains("\"activeProfile\""));
    assert!(raw.ends_with("\n"), "trailing newline");
    let _ = std::fs::remove_dir_all(&dir);
}
