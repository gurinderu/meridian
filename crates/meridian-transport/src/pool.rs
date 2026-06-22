use std::sync::Mutex;

/// Descriptor for spawning a process: the account and an optional resume token.
/// (Was also the key of a warm-idle reuse set — removed: process reuse is now
/// session-keyed in `ParkedStore` [meridian crate], because a `claude` process
/// is stateful per session and reusing one for a DIFFERENT conversation would
/// leak context. The pool is purely a concurrency gate + spawner now.)
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

/// A concurrency gate: bounds the number of live processes at `global_cap` and
/// spawns one per `acquire`. It does NOT recycle processes (see `IsolationKey`).
pub struct Pool<F: ProcessFactory> {
    factory: F,
    global_cap: usize,
    live: Mutex<usize>,
}

/// Held for the duration of one turn. On drop, the process is dropped and its
/// global-cap slot is freed. To keep the process alive (to park it), take it
/// out with `take_proc` first.
pub struct Lease<'a, F: ProcessFactory> {
    pool: &'a Pool<F>,
    proc: Option<F::Proc>,
}

impl<F: ProcessFactory> Pool<F> {
    pub fn new(factory: F, global_cap: usize) -> Self {
        Pool { factory, global_cap, live: Mutex::new(0) }
    }

    pub fn live_count(&self) -> usize {
        *self.live.lock().unwrap()
    }

    pub async fn acquire(&self, key: &IsolationKey) -> std::io::Result<Option<Lease<'_, F>>> {
        // Reserve a slot under the lock (never held across the await).
        {
            let mut live = self.live.lock().unwrap();
            if *live >= self.global_cap {
                return Ok(None);
            }
            *live += 1;
        }
        // Spawn outside the lock; on failure, give the slot back.
        match self.factory.spawn(key).await {
            Ok(p) => Ok(Some(Lease { pool: self, proc: Some(p) })),
            Err(e) => {
                self.drop_one();
                Err(e)
            }
        }
    }

    fn drop_one(&self) {
        *self.live.lock().unwrap() -= 1;
    }
}

impl<'a, F: ProcessFactory> Lease<'a, F> {
    pub fn proc(&mut self) -> &mut F::Proc {
        self.proc.as_mut().expect("lease still holds a process")
    }

    /// Take the process OUT of this lease and return it to the caller (e.g. to
    /// park it). The process leaves the pool's management, so we free its
    /// global-cap slot HERE — Drop then sees `proc == None` and does nothing.
    /// Must only be called once; returns None if already taken.
    pub fn take_proc(&mut self) -> Option<F::Proc> {
        let p = self.proc.take()?;
        self.pool.drop_one();
        Some(p)
    }
}

impl<'a, F: ProcessFactory> Drop for Lease<'a, F> {
    fn drop(&mut self) {
        if let Some(p) = self.proc.take() {
            self.pool.drop_one();
            drop(p);
        }
    }
}
