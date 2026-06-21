//! Session-sticky process park. Keeps a live `claude` process per conversation
//! `(profile_id, session_id)` so a continuation turn reuses it instead of
//! cold-spawning. Bounded by an LRU cap (park) + an idle TTL (reap); generic
//! over the process type for testability.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry<P> {
    proc: P,
    last_used: Instant,
}

#[derive(Default)]
pub struct ParkedStore<P> {
    inner: Mutex<HashMap<(String, String), Entry<P>>>,
}

impl<P> ParkedStore<P> {
    pub fn new() -> Self {
        ParkedStore { inner: Mutex::new(HashMap::new()) }
    }

    pub fn take(&self, profile_id: &str, session_id: &str) -> Option<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(&(profile_id.to_string(), session_id.to_string())).map(|e| e.proc)
    }

    /// Insert; evict LRU entries over `max_parked` and return the evicted procs
    /// (the caller shuts them down).
    pub fn park(&self, profile_id: String, session_id: String, proc: P, max_parked: usize) -> Vec<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.insert((profile_id, session_id), Entry { proc, last_used: Instant::now() });
        let mut evicted = Vec::new();
        while g.len() > max_parked.max(1) {
            // find the least-recently-used key
            let Some(lru) = g.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) else { break };
            if let Some(e) = g.remove(&lru) {
                evicted.push(e.proc);
            }
        }
        evicted
    }

    pub fn reap(&self, ttl: Duration) -> Vec<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let stale: Vec<(String, String)> = g.iter()
            .filter(|(_, e)| now.duration_since(e.last_used) > ttl)
            .map(|(k, _)| k.clone())
            .collect();
        stale.into_iter().filter_map(|k| g.remove(&k).map(|e| e.proc)).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}
