use meridian::token_refresh::*;

#[test]
fn serialize_is_compact() {
    let c: CredentialsFile = serde_json::from_str(
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":111}}"#).unwrap();
    let s = serialize_credentials(&c);
    assert!(!s.contains('\n') && !s.contains("  "), "must be compact (issue #452): {s}");
    assert!(s.contains("\"claudeAiOauth\"") && s.contains("\"accessToken\":\"a\""));
}

#[test]
fn unknown_keys_preserved_on_roundtrip() {
    let c: CredentialsFile = serde_json::from_str(
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1},"other":42}"#).unwrap();
    assert!(serialize_credentials(&c).contains("\"other\":42"));
}

#[test]
fn sha256_known_vector() {
    // echo -n abc | sha256sum
    assert_eq!(sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn keychain_service_default_vs_profile() {
    let home = std::env::var("HOME").unwrap();
    let default_dir = format!("{home}/.claude");
    assert_eq!(config_dir_to_keychain_service(&default_dir), "Claude Code-credentials");
    let other = config_dir_to_keychain_service("/some/profile/dir");
    assert!(other.starts_with("Claude Code-credentials-") && other.len() == "Claude Code-credentials-".len() + 8);
}

#[test]
fn credentials_file_path_for_profile() {
    assert_eq!(config_dir_to_credentials_file("/p/dir"),
        std::path::PathBuf::from("/p/dir/.credentials.json"));
}

#[test]
fn file_store_roundtrip_and_ensure_fresh() {
    let dir = std::env::temp_dir().join(format!("mer-tok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = create_platform_credential_store(Some(dir.to_str().unwrap()));
    // create_platform_credential_store on macOS returns a KeychainStore for a
    // profile dir — so test the FileStore directly instead for determinism:
    let path = dir.join(".credentials.json");
    let fs: Box<dyn CredentialStore> = Box::new(FileStore::new(path));
    let future = (now_ms() + 10 * 60 * 1000) as i64;
    let creds: CredentialsFile = serde_json::from_str(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"a","refreshToken":"r","expiresAt":{future}}}}}"#)).unwrap();
    assert!(fs.write(&creds));
    let back = fs.read().unwrap();
    assert_eq!(back.claude_ai_oauth.access_token, "a");
    // ensure_fresh_token: token valid for >buffer -> true WITHOUT any network.
    assert!(ensure_fresh_token(fs.as_ref(), 5 * 60 * 1000));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = store; // silence unused on non-macos
}

#[cfg(unix)]
#[test]
fn file_store_writes_credentials_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("mer-tok-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(".credentials.json");
    let store = meridian::token_refresh::FileStore::new(path.clone());
    let creds: meridian::token_refresh::CredentialsFile = serde_json::from_str(
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1}}"#).unwrap();
    use meridian::token_refresh::CredentialStore;
    assert!(store.write(&creds));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "credentials must be owner-only, got {mode:o}");
    let _ = std::fs::remove_dir_all(&dir);
}
