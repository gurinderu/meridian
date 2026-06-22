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

/// Default cap on the in-memory fingerprint→session map. ~80 bytes/entry, so
/// the bound is ~1 MB. Eviction of a paused conversation's entry just makes its
/// next turn cold-resume from disk (worst case == no-resume) — never incorrect.
const DEFAULT_SESSION_CAP: usize = 10_000;

struct Inner {
    /// fingerprint → (session_id, insertion sequence). The sequence gives an
    /// insertion-order LRU: when over cap, the smallest-seq entry is evicted.
    map: HashMap<String, (String, u64)>,
    seq: u64,
}

/// In-memory, capped map from conversation fingerprint to `claude` session id.
/// Bounded so a long-running proxy can't grow it without limit (every turn
/// inserts a new fingerprint); insertion-order LRU drops the oldest entries.
pub struct SessionStore {
    inner: Mutex<Inner>,
    cap: usize,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::with_cap(DEFAULT_SESSION_CAP)
    }
    pub fn with_cap(cap: usize) -> Self {
        SessionStore { inner: Mutex::new(Inner { map: HashMap::new(), seq: 0 }), cap: cap.max(1) }
    }
    pub fn get(&self, key: &str) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).map.get(key).map(|(sid, _)| sid.clone())
    }
    pub fn insert(&self, key: String, session_id: String) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.seq += 1;
        let seq = g.seq;
        g.map.insert(key, (session_id, seq));
        // Evict the oldest (smallest-seq) entries while over cap. cap is small
        // (default 10k), so the min-scan per overflow insert is negligible.
        while g.map.len() > self.cap {
            let Some(oldest) = g.map.iter().min_by_key(|(_, (_, s))| *s).map(|(k, _)| k.clone()) else { break };
            g.map.remove(&oldest);
        }
    }
    /// Evict every cached session. Called when the active profile changes:
    /// sessions were started under the previous account's credentials and
    /// must not be resumed under a different identity.
    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map.clear();
        g.seq = 0;
    }
    /// Number of stored sessions. For tests asserting a session was recorded.
    pub fn len_for_test(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).map.len()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
