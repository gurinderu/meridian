use std::collections::HashMap;
use std::sync::Mutex;

/// Pool partition key. A process is only reused for an identical key.
/// (Earlier `cwd`/`options_hash` fields were dropped: they were always the same
/// constant at every call site and were never applied to the spawned process —
/// the pool advertised an isolation it did not enforce. Re-add them only
/// alongside the matching `SpawnConfig`/`current_dir` wiring.)
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct IsolationKey {
    pub profile_id: String,
    pub resume: Option<String>,
}

pub trait ProcessFactory: Send + Sync {
    type Proc: Send;
    fn spawn(
        &self,
        key: &IsolationKey,
    ) -> impl std::future::Future<Output = std::io::Result<Self::Proc>> + Send;
}

struct Inner<P> {
    idle: HashMap<IsolationKey, Vec<P>>,
    live: usize,
}

pub struct Pool<F: ProcessFactory> {
    factory: F,
    global_cap: usize,
    inner: Mutex<Inner<F::Proc>>,
}

/// Held for the duration of one `query()`. On drop, the process is returned
/// to the warm idle set for its key (recycle policy lands in a later phase).
pub struct Lease<'a, F: ProcessFactory> {
    pool: &'a Pool<F>,
    key: IsolationKey,
    proc: Option<F::Proc>,
    discard: bool,
}

impl<F: ProcessFactory> Pool<F> {
    pub fn new(factory: F, global_cap: usize) -> Self {
        Pool { factory, global_cap, inner: Mutex::new(Inner { idle: HashMap::new(), live: 0 }) }
    }

    pub fn live_count(&self) -> usize {
        self.inner.lock().unwrap().live
    }

    pub async fn acquire(&self, key: &IsolationKey) -> std::io::Result<Option<Lease<'_, F>>> {
        // Phase 1: lock, decide, never hold the lock across the await.
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(p) = g.idle.get_mut(key).and_then(Vec::pop) {
                g.live += 1;
                return Ok(Some(Lease { pool: self, key: key.clone(), proc: Some(p), discard: false }));
            }
            if g.live >= self.global_cap {
                return Ok(None);
            }
            g.live += 1; // reserve the slot before releasing the lock
        }
        // Phase 2: spawn outside the lock; on failure, give the slot back.
        match self.factory.spawn(key).await {
            Ok(p) => Ok(Some(Lease { pool: self, key: key.clone(), proc: Some(p), discard: false })),
            Err(e) => {
                self.inner.lock().unwrap().live -= 1;
                Err(e)
            }
        }
    }

    fn release(&self, key: IsolationKey, proc: F::Proc) {
        let mut g = self.inner.lock().unwrap();
        g.live -= 1;
        g.idle.entry(key).or_default().push(proc);
    }

    fn drop_one(&self) {
        self.inner.lock().unwrap().live -= 1;
    }
}

impl<'a, F: ProcessFactory> Lease<'a, F> {
    pub fn proc(&mut self) -> &mut F::Proc {
        self.proc.as_mut().expect("lease still holds a process")
    }

    /// Mark this lease so its process is NOT returned to the warm idle set on
    /// drop. Use after the process has been shut down / is no longer reusable.
    /// The global-cap slot is still freed.
    pub fn discard(&mut self) {
        self.discard = true;
    }

    /// Take the process OUT of this lease and return it to the caller (e.g. to
    /// park it). The process leaves the pool's management, so we free its
    /// global-cap slot HERE — Drop then sees `proc == None` and does nothing.
    /// (Freeing only via `discard` would NOT work: Drop's `if let Some(p)` is
    /// skipped once `proc` is None, so `drop_one` would never run and the slot
    /// would leak.) Must only be called once; returns None if already taken.
    pub fn take_proc(&mut self) -> Option<F::Proc> {
        let p = self.proc.take()?;
        self.pool.drop_one();
        Some(p)
    }
}

impl<'a, F: ProcessFactory> Drop for Lease<'a, F> {
    fn drop(&mut self) {
        if let Some(p) = self.proc.take() {
            if self.discard {
                self.pool.drop_one();
                drop(p);
            } else {
                self.pool.release(self.key.clone(), p);
            }
        }
    }
}
