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
    /// (the caller shuts them down). A same-key re-park returns the displaced
    /// process too, so it gets a graceful shutdown rather than a silent drop.
    pub fn park(&self, profile_id: String, session_id: String, proc: P, max_parked: usize) -> Vec<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut evicted = Vec::new();
        if let Some(old) = g.insert((profile_id, session_id), Entry { proc, last_used: Instant::now() }) {
            evicted.push(old.proc);
        }
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

    /// Snapshot each parked entry's `(profile, session)` key, its last-used
    /// instant, and an arbitrary cheap attribute via `attr_of` (e.g. the pid).
    /// Taken under the lock but `attr_of` must NOT do I/O — read the heavy thing
    /// (RSS) outside the lock, then evict the chosen keys with `take`.
    pub fn snapshot<M>(&self, attr_of: impl Fn(&P) -> M) -> Vec<((String, String), Instant, M)> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().map(|(k, e)| (k.clone(), e.last_used, attr_of(&e.proc))).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

/// Pure: given parked entries `(key, last_used, rss_bytes)`, return the keys to
/// evict — oldest-first until the summed RSS of the remainder is within
/// `budget`. Empty when already within budget. Decoupled from the store + any
/// I/O so it's unit-testable; the caller reads RSS (outside the lock) and then
/// `take`s the returned keys.
pub fn over_budget_evictions(
    mut items: Vec<((String, String), Instant, u64)>,
    budget: u64,
) -> Vec<(String, String)> {
    let mut total: u64 = items.iter().map(|(_, _, r)| *r).sum();
    if total <= budget {
        return Vec::new();
    }
    items.sort_by_key(|(_, t, _)| *t); // oldest first
    let mut evict = Vec::new();
    for (k, _, r) in items {
        if total <= budget {
            break;
        }
        evict.push(k);
        total = total.saturating_sub(r);
    }
    evict
}
