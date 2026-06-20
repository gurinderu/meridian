//! Persistent server settings (~/.config/meridian/settings.json). Survives
//! restarts. Leaf module — no imports from server/session/profiles.
//! Port of src-original/src/proxy/settings.ts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MeridianSettings {
	/// Last active profile ID — restored on proxy startup.
	#[serde(rename = "activeProfile", default, skip_serializing_if = "Option::is_none")]
	pub active_profile: Option<String>,
}

pub fn settings_path() -> Option<PathBuf> {
	let home = std::env::var("HOME").ok()?;
	Some(PathBuf::from(home).join(".config").join("meridian").join("settings.json"))
}

/// Read settings. Returns default (empty) on a missing or invalid file.
pub fn load_settings_at(path: &Path) -> MeridianSettings {
	let Ok(raw) = std::fs::read_to_string(path) else { return MeridianSettings::default() };
	serde_json::from_str(&raw).unwrap_or_default()
}

/// Merge `updates` into the existing file (preserving unknown keys) and write
/// back with mode 0o600, pretty JSON, trailing newline.
pub fn save_settings_at(path: &Path, updates: MeridianSettings) -> std::io::Result<()> {
	// Start from whatever is on disk as a generic object so unknown keys survive.
	let mut obj: Map<String, Value> = std::fs::read_to_string(path)
		.ok()
		.and_then(|r| serde_json::from_str(&r).ok())
		.unwrap_or_default();
	if let Value::Object(up) = serde_json::to_value(&updates).unwrap_or(Value::Null) {
		for (k, v) in up {
			obj.insert(k, v);
		}
	}
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	let mut body = serde_json::to_string_pretty(&Value::Object(obj))?;
	body.push('\n');
	write_private(path, body.as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
	use std::os::unix::fs::OpenOptionsExt;
	use std::io::Write;
	let mut f = std::fs::OpenOptions::new()
		.write(true).create(true).truncate(true).mode(0o600).open(path)?;
	f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
	std::fs::write(path, bytes)
}

pub fn get_active_profile() -> Option<String> {
	settings_path().map(|p| load_settings_at(&p)).and_then(|s| s.active_profile)
}

pub fn set_active_profile(id: &str) {
	let Some(path) = settings_path() else { return };
	if let Err(e) = save_settings_at(&path, MeridianSettings { active_profile: Some(id.to_string()) }) {
		tracing::warn!("failed to persist active profile to {}: {e}", path.display());
	}
}
