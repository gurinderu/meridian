//! Multi-profile support. Each profile is a named auth context — a
//! CLAUDE_CONFIG_DIR (claude-max), an Anthropic API key (api), or a long-lived
//! OAuth token (oauth-token). Selection priority: x-meridian-profile header >
//! active profile > first configured profile > implicit "default".
//! Port of `src-original/src/proxy/profiles.ts` (request-path subset).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use meridian_transport::factory::EnvResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileType {
    ClaudeMax,
    Api,
    OauthToken,
}

#[derive(Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub id: String,
    pub kind: ProfileType,
    pub is_active: bool,
}

const DEFAULT_PROFILE_ID: &str = "default";
const DISK_CACHE_TTL_MS: u128 = 5_000;

pub struct ProfileStore {
    config_profiles: Vec<ProfileConfig>,
    config_root: PathBuf,
    active: Mutex<Option<String>>,
    disk_discovery: bool,
    disk_cache: Mutex<Option<(std::time::Instant, Vec<ProfileConfig>)>>,
}

impl ProfileStore {
    pub fn new(profiles: Vec<ProfileConfig>, config_root: PathBuf) -> Self {
        ProfileStore {
            config_profiles: profiles,
            config_root,
            active: Mutex::new(None),
            disk_discovery: false,
            disk_cache: Mutex::new(None),
        }
    }

    /// Load from `MERIDIAN_PROFILES` (JSON array) or `~/.config/meridian/profiles.json`.
    pub fn from_env_or_disk(config_root: PathBuf) -> Self {
        let profiles = load_profiles().unwrap_or_default();
        Self::new(profiles, config_root)
    }

    /// Turn on live re-discovery of ~/.config/meridian/profiles.json (5s TTL).
    pub fn with_disk_discovery(mut self) -> Self {
        self.disk_discovery = true;
        self
    }

    fn disk_profiles(&self) -> Vec<ProfileConfig> {
        let mut g = self.disk_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, ref cached)) = *g {
            if at.elapsed().as_millis() < DISK_CACHE_TTL_MS {
                return cached.clone();
            }
        }
        let fresh = profiles_json_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| match serde_json::from_str::<Vec<ProfileConfig>>(&raw) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("profiles.json is not valid JSON: {e}; ignoring");
                    None
                }
            })
            .unwrap_or_default();
        *g = Some((std::time::Instant::now(), fresh.clone()));
        fresh
    }

    pub fn effective(&self) -> Vec<ProfileConfig> {
        if !self.disk_discovery {
            return self.config_profiles.clone();
        }
        merge_effective(&self.config_profiles, self.disk_profiles())
    }

    pub fn list(&self) -> Vec<ProfileSummary> {
        let eff = self.effective();
        if eff.is_empty() {
            return vec![];
        }
        // Resolve the active id by the SAME precedence as resolve_id(None):
        // a stored active that is no longer in the effective list falls back to
        // the first profile. Without this, a stale active id would leave every
        // row is_active=false while resolve_id(None) (the response's
        // activeProfile) reports the first profile — an inconsistent view.
        let active = match self.active() {
            Some(a) if eff.iter().any(|p| p.id == a) => a,
            _ => eff[0].id.clone(),
        };
        eff.iter().map(|p| ProfileSummary {
            id: p.id.clone(),
            kind: self.resolved_type_of(p),
            is_active: p.id == active,
        }).collect()
    }

    pub fn restore_active(&self) {
        if self.active().is_some() {
            return;
        }
        if !self.disk_discovery {
            return;
        }
        let Some(saved) = crate::settings::get_active_profile() else { return };
        let eff = self.effective();
        if eff.is_empty() || eff.iter().any(|p| p.id == saved) {
            *self.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(saved);
        } else {
            tracing::warn!("saved active profile \"{saved}\" not found; using default");
        }
    }

    /// Set the process-wide active profile. NOT safe to drive from per-request
    /// HTTP handlers: it is shared mutable state across all concurrent requests.
    /// Request-scoped selection must go through the `x-meridian-profile` header
    /// (resolve_id), which supersedes this. Intended for a future CLI/management
    /// command that sets a session default.
    pub fn set_active(&self, id: String) {
        if self.disk_discovery {
            crate::settings::set_active_profile(&id);
        }
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
    }

    pub fn active(&self) -> Option<String> {
        self.active.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn find_in<'a>(eff: &'a [ProfileConfig], id: &str) -> Option<&'a ProfileConfig> {
        eff.iter().find(|p| p.id == id)
    }

    /// Resolve a request to a profile id: header > active > first > "default".
    /// An unknown header/active id falls back to the first profile.
    pub fn resolve_id(&self, requested: Option<&str>) -> String {
        let eff = self.effective();
        if eff.is_empty() {
            return DEFAULT_PROFILE_ID.to_string();
        }
        let first = eff[0].id.clone();
        let candidate = requested
            .map(str::to_string)
            .or_else(|| self.active())
            .unwrap_or_else(|| first.clone());
        if Self::find_in(&eff, &candidate).is_some() {
            candidate
        } else {
            tracing::warn!("unknown profile \"{candidate}\"; using first profile \"{first}\"");
            first
        }
    }

    fn resolved_type_of(&self, p: &ProfileConfig) -> ProfileType {
        if p.oauth_token.is_some() || p.kind == Some(ProfileType::OauthToken) {
            ProfileType::OauthToken
        } else {
            p.kind.unwrap_or(ProfileType::ClaudeMax)
        }
    }

    pub fn resolved_type(&self, id: &str) -> ProfileType {
        let eff = self.effective();
        match Self::find_in(&eff, id) {
            Some(p) => self.resolved_type_of(p),
            None => ProfileType::ClaudeMax,
        }
    }

    pub fn config_dir_for(&self, id: &str) -> Option<String> {
        let eff = self.effective();
        Self::find_in(&eff, id).and_then(|p| p.claude_config_dir.clone())
    }

    fn overlay_for(&self, id: &str) -> HashMap<String, String> {
        let eff = self.effective();
        let Some(p) = Self::find_in(&eff, id) else { return HashMap::new() };
        let mut env = HashMap::new();
        match self.resolved_type_of(p) {
            ProfileType::OauthToken => {
                if let Some(tok) = &p.oauth_token {
                    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), tok.clone());
                    // Isolate from host ~/.claude (profiles.ts:201). Sanitize the
                    // id: it comes from MERIDIAN_PROFILES / profiles.json (not
                    // validated on load), and an id like `../x` would otherwise
                    // escape config_root into an arbitrary CLAUDE_CONFIG_DIR. Must
                    // match dirs_to_remove_on_remove's join so removal targets the
                    // same dir.
                    let dir = self.config_root.join("profiles")
                        .join(meridian_transport::factory::safe_profile_segment(&p.id));
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

/// Config profiles followed by disk profiles whose id is not already present.
pub fn merge_effective(from_config: &[ProfileConfig], from_disk: Vec<ProfileConfig>) -> Vec<ProfileConfig> {
    let ids: std::collections::HashSet<&str> = from_config.iter().map(|p| p.id.as_str()).collect();
    let mut out = from_config.to_vec();
    out.extend(from_disk.into_iter().filter(|p| !ids.contains(p.id.as_str())));
    out
}

fn profiles_json_path() -> Option<PathBuf> {
    dirs_config_meridian().map(|d| d.join("profiles.json"))
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
