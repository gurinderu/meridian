//! Pure helpers for the `meridian profile` CLI. Reads/writes
//! ~/.config/meridian/profiles.json. Port of the management subset of
//! src-original/src/proxy/profileCli.ts (browser OAuth login is Phase 3d).

use std::path::{Path, PathBuf};
use crate::profiles::ProfileConfig;

pub fn is_valid_profile_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn config_meridian() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config").join("meridian"))
}
pub fn profiles_json_path() -> Option<PathBuf> { config_meridian().map(|d| d.join("profiles.json")) }
pub fn profiles_dir() -> Option<PathBuf> { config_meridian().map(|d| d.join("profiles")) }

pub fn load_profiles_json_at(path: &Path) -> Vec<ProfileConfig> {
    let Ok(raw) = std::fs::read_to_string(path) else { return vec![] };
    match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => { tracing::warn!("failed to read {}: {e}", path.display()); vec![] }
    }
}

pub fn save_profiles_json_at(path: &Path, profiles: &[ProfileConfig]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut body = serde_json::to_string_pretty(profiles)?;
    body.push('\n');
    write_private(path, body.as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    // mode(0o600) only applies on create — re-assert in case profiles.json (which
    // persists OAuth tokens / API keys) pre-existed with looser permissions.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)
}
#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> { std::fs::write(path, bytes) }

/// Directories to delete when a profile is removed (port of dirsToRemoveOnProfileRemove).
pub fn dirs_to_remove_on_remove(p: &ProfileConfig, profiles_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cd) = &p.claude_config_dir {
        let candidate = Path::new(cd);
        // `starts_with` is purely lexical and does not collapse `..`, so a
        // crafted claudeConfigDir like `<profiles_dir>/../../etc` would slip the
        // containment guard and be handed to remove_dir_all. Reject any path
        // with a parent-dir component so containment is actually enforced.
        let has_traversal = candidate.components().any(|c| c == std::path::Component::ParentDir);
        if !has_traversal && candidate.starts_with(profiles_dir) {
            dirs.push(PathBuf::from(cd));
        }
    }
    let is_oauth = p.oauth_token.is_some()
        || p.kind == Some(crate::profiles::ProfileType::OauthToken);
    if is_oauth {
        // Sanitize the id to match overlay_for's isolation-dir join (profiles.rs)
        // — both must produce the same single safe segment so removal deletes the
        // dir that was actually created, and never escapes profiles_dir.
        let iso = profiles_dir.join(meridian_transport::factory::safe_profile_segment(&p.id));
        if !dirs.contains(&iso) { dirs.push(iso); }
    }
    dirs
}

pub fn add_oauth_token(path: &Path, id: &str, token: &str) -> Result<(), String> {
    if !is_valid_profile_id(id) {
        return Err("Invalid profile ID. Use only letters, numbers, hyphens, underscores.".into());
    }
    let mut profiles = load_profiles_json_at(path);
    if profiles.iter().any(|p| p.id == id) {
        return Err(format!("Profile \"{id}\" already exists."));
    }
    if token.trim().is_empty() { return Err("Empty token. Aborted.".into()); }
    profiles.push(ProfileConfig {
        id: id.to_string(),
        kind: Some(crate::profiles::ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None,
        oauth_token: Some(token.trim().to_string()),
    });
    save_profiles_json_at(path, &profiles).map_err(|e| e.to_string())
}
