//! Multi-profile support. Each profile is a named auth context — a
//! CLAUDE_CONFIG_DIR (claude-max), an Anthropic API key (api), or a long-lived
//! OAuth token (oauth-token). Selection priority: x-meridian-profile header >
//! active profile > first configured profile > implicit "default".
//! Port of `src-original/src/proxy/profiles.ts` (request-path subset).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;

use meridian_transport::factory::EnvResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileType {
    ClaudeMax,
    Api,
    OauthToken,
}

#[derive(Clone, Deserialize)]
pub struct ProfileConfig {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<ProfileType>,
    #[serde(rename = "claudeConfigDir", default)]
    pub claude_config_dir: Option<String>,
    #[serde(rename = "apiKey", default)]
    pub api_key: Option<String>,
    #[serde(rename = "baseUrl", default)]
    pub base_url: Option<String>,
    #[serde(rename = "oauthToken", default)]
    pub oauth_token: Option<String>,
}

/// Manual `Debug` that redacts the secret-bearing fields so a stray
/// `{:?}` / error chain can never print an api key or OAuth token.
impl std::fmt::Debug for ProfileConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |o: &Option<String>| o.as_ref().map(|_| "[redacted]");
        f.debug_struct("ProfileConfig")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("claude_config_dir", &self.claude_config_dir)
            .field("api_key", &redact(&self.api_key))
            .field("base_url", &self.base_url)
            .field("oauth_token", &redact(&self.oauth_token))
            .finish()
    }
}

const DEFAULT_PROFILE_ID: &str = "default";

pub struct ProfileStore {
    profiles: Vec<ProfileConfig>,
    config_root: PathBuf,
    active: Mutex<Option<String>>,
}

impl ProfileStore {
    pub fn new(profiles: Vec<ProfileConfig>, config_root: PathBuf) -> Self {
        ProfileStore { profiles, config_root, active: Mutex::new(None) }
    }

    /// Load from `MERIDIAN_PROFILES` (JSON array) or `~/.config/meridian/profiles.json`.
    pub fn from_env_or_disk(config_root: PathBuf) -> Self {
        let profiles = load_profiles().unwrap_or_default();
        Self::new(profiles, config_root)
    }

    /// Set the process-wide active profile. NOT safe to drive from per-request
    /// HTTP handlers: it is shared mutable state across all concurrent requests.
    /// Request-scoped selection must go through the `x-meridian-profile` header
    /// (resolve_id), which supersedes this. Intended for a future CLI/management
    /// command that sets a session default.
    pub fn set_active(&self, id: String) {
        *self.active.lock().unwrap() = Some(id);
    }

    pub fn active(&self) -> Option<String> {
        self.active.lock().unwrap().clone()
    }

    fn find(&self, id: &str) -> Option<&ProfileConfig> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Resolve a request to a profile id: header > active > first > "default".
    /// An unknown header/active id falls back to the first profile.
    pub fn resolve_id(&self, requested: Option<&str>) -> String {
        if self.profiles.is_empty() {
            return DEFAULT_PROFILE_ID.to_string();
        }
        let first = self.profiles[0].id.clone();
        let candidate = requested
            .map(str::to_string)
            .or_else(|| self.active())
            .unwrap_or_else(|| first.clone());
        if self.find(&candidate).is_some() {
            candidate
        } else {
            tracing::warn!("unknown profile \"{candidate}\"; using first profile \"{first}\"");
            first
        }
    }

    pub fn resolved_type(&self, id: &str) -> ProfileType {
        match self.find(id) {
            Some(p) if p.oauth_token.is_some() || p.kind == Some(ProfileType::OauthToken) => ProfileType::OauthToken,
            Some(p) => p.kind.unwrap_or(ProfileType::ClaudeMax),
            None => ProfileType::ClaudeMax,
        }
    }

    fn overlay_for(&self, id: &str) -> HashMap<String, String> {
        let Some(p) = self.find(id) else { return HashMap::new() };
        let mut env = HashMap::new();
        match self.resolved_type(id) {
            ProfileType::OauthToken => {
                if let Some(tok) = &p.oauth_token {
                    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), tok.clone());
                    // Isolate from host ~/.claude (profiles.ts:201).
                    let dir = self.config_root.join("profiles").join(&p.id);
                    env.insert("CLAUDE_CONFIG_DIR".into(), dir.to_string_lossy().into_owned());
                } else {
                    tracing::warn!(
                        "profile \"{}\" is type oauth-token but has no oauthToken; \
                         it will fall back to host auth",
                        p.id
                    );
                }
            }
            ProfileType::Api => {
                if let Some(k) = &p.api_key {
                    env.insert("ANTHROPIC_API_KEY".into(), k.clone());
                }
                if let Some(b) = &p.base_url {
                    env.insert("ANTHROPIC_BASE_URL".into(), b.clone());
                }
            }
            ProfileType::ClaudeMax => {
                if let Some(d) = &p.claude_config_dir {
                    env.insert("CLAUDE_CONFIG_DIR".into(), d.clone());
                }
            }
        }
        env
    }
}

impl EnvResolver for ProfileStore {
    fn overlay(&self, profile_id: &str) -> HashMap<String, String> {
        self.overlay_for(profile_id)
    }
}

fn load_profiles() -> Option<Vec<ProfileConfig>> {
    if let Ok(raw) = std::env::var("MERIDIAN_PROFILES") {
        return match serde_json::from_str(&raw) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("MERIDIAN_PROFILES is not valid JSON: {e}; ignoring (no profiles)");
                None
            }
        };
    }
    let path = dirs_config_meridian()?.join("profiles.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("{} is not valid JSON: {e}; ignoring (no profiles)", path.display());
            None
        }
    }
}

fn dirs_config_meridian() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("meridian"))
}
