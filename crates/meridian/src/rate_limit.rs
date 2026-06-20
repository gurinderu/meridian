//! In-memory snapshot of the Claude Max subscription quota, captured from the
//! CLI's `rate_limit_event` stream messages. One bucket per `rateLimitType`
//! (last-write-wins), plus an internal "default" bucket for events that omit
//! it. Port of src-original/src/proxy/rateLimitStore.ts (SDK-sourced subset).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

const DEFAULT_BUCKET: &str = "default";

struct Entry {
    info: Value,
    observed_at: u64,
}

#[derive(Default)]
pub struct RateLimitStore {
    entries: Mutex<HashMap<String, Entry>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RateLimitStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a `rate_limit_info` snapshot. No-op for non-objects.
    pub fn record(&self, info: &Value) {
        if !info.is_object() {
            return;
        }
        let key = info.get("rateLimitType").and_then(Value::as_str).unwrap_or(DEFAULT_BUCKET).to_string();
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Entry { info: info.clone(), observed_at: now_ms() });
    }

    /// Real (non-default) buckets, newest-first by observedAt.
    pub fn get_all(&self) -> Vec<Value> {
        let g = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(u64, Value)> = g
            .iter()
            .filter(|(k, _)| k.as_str() != DEFAULT_BUCKET)
            .map(|(_, e)| (e.observed_at, bucket_to_json(&e.info, e.observed_at)))
            .collect();
        out.sort_by_key(|b| std::cmp::Reverse(b.0));
        out.into_iter().map(|(_, v)| v).collect()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).keys().filter(|k| k.as_str() != DEFAULT_BUCKET).count()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Normalize a `rate_limit_info` object into the wire bucket shape (verbatim
/// field set from the TS original), with JSON null fallbacks.
pub fn bucket_to_json(info: &Value, observed_at: u64) -> Value {
    let f = |k: &str| info.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "type": f("rateLimitType"),
        "status": f("status"),
        "utilization": f("utilization"),
        "resetsAt": f("resetsAt"),
        "isUsingOverage": info.get("isUsingOverage").and_then(Value::as_bool).unwrap_or(false),
        "overageStatus": f("overageStatus"),
        "overageResetsAt": f("overageResetsAt"),
        "overageDisabledReason": f("overageDisabledReason"),
        "surpassedThreshold": f("surpassedThreshold"),
        "observedAt": observed_at,
    })
}
