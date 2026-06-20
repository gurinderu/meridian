use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use serde_json::Value;

fn message_text(m: &Value) -> String {
    message_text_pub(m)
}

/// Extract the text content from a message value.
pub fn message_text_pub(m: &Value) -> String {
    match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Stable content hash of a conversation prefix, over each `(role, text)`.
pub fn fingerprint(prefix: &[Value]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for m in prefix {
        m.get("role").and_then(Value::as_str).unwrap_or("").hash(&mut hasher);
        message_text(m).hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// In-memory map from conversation fingerprint to `claude` session id.
pub struct SessionStore {
    inner: Mutex<HashMap<String, String>>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore { inner: Mutex::new(HashMap::new()) }
    }
    pub fn get(&self, key: &str) -> Option<String> {
        self.inner.lock().unwrap().get(key).cloned()
    }
    pub fn insert(&self, key: String, session_id: String) {
        self.inner.lock().unwrap().insert(key, session_id);
    }
    /// Evict every cached session. Called when the active profile changes:
    /// sessions were started under the previous account's credentials and
    /// must not be resumed under a different identity.
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
