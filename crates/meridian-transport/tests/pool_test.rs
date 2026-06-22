use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use meridian_transport::pool::{IsolationKey, Pool, ProcessFactory};

struct CountingFactory { spawned: Arc<AtomicUsize>, fail: bool }
struct FakeProc { #[allow(dead_code)] id: usize }

impl ProcessFactory for CountingFactory {
    type Proc = FakeProc;
    async fn spawn(&self, _k: &IsolationKey) -> std::io::Result<FakeProc> {
        if self.fail {
            return Err(std::io::Error::other("spawn failed"));
        }
        Ok(FakeProc { id: self.spawned.fetch_add(1, Ordering::SeqCst) })
    }
}

fn key(p: &str) -> IsolationKey {
    IsolationKey { profile_id: p.into(), resume: None }
}

#[tokio::test]
async fn each_acquire_spawns_a_fresh_process() {
    // The pool does NOT recycle processes (reuse is session-keyed in ParkedStore),
    // so two acquires of the same key spawn two processes; the first lease frees
    // its slot on drop.
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 4);
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(pool.live_count(), 0, "lease drop frees the slot");
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2, "no warm reuse — each acquire spawns");
}

#[tokio::test]
async fn different_keys_do_not_share() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 4);
    { let _a = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    { let _b = pool.acquire(&key("b")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn global_cap_blocks_when_all_leases_live() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 1);
    let _l1 = pool.acquire(&key("a")).await.unwrap().unwrap();
    assert!(pool.acquire(&key("b")).await.unwrap().is_none());
    assert_eq!(pool.live_count(), 1);
}

#[tokio::test]
async fn spawn_failure_does_not_leak_a_cap_slot() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned, fail: true }, 1);
    assert!(pool.acquire(&key("a")).await.is_err());
    assert_eq!(pool.live_count(), 0, "failed spawn must not consume the slot");
    // The single slot is still available after the failure:
    assert!(pool.acquire(&key("a")).await.is_err()); // still fails, but slot reusable each time
    assert_eq!(pool.live_count(), 0);
}

#[tokio::test]
async fn dropped_lease_frees_slot_and_is_not_reused() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 4);
    {
        let _l = pool.acquire(&key("a")).await.unwrap().unwrap();
    } // drop frees the slot and drops the process (never recycled)
    assert_eq!(pool.live_count(), 0, "lease drop frees the cap slot");
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2, "a dropped process must not be reused");
}

#[tokio::test]
async fn take_proc_frees_slot_exactly_once_and_yields_the_process() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 1);
    let taken = {
        let mut l = pool.acquire(&key("a")).await.unwrap().unwrap();
        assert_eq!(pool.live_count(), 1);
        let p = l.take_proc();          // process leaves pool management
        assert!(p.is_some());
        // slot is freed immediately by take_proc, before the lease drops
        assert_eq!(pool.live_count(), 0, "take_proc frees the cap slot");
        p
        // lease drops here with proc=None -> must NOT double-free / underflow
    };
    assert!(taken.is_some());
    assert_eq!(pool.live_count(), 0, "no double-free on drop after take_proc");
    // the single slot is reusable, and the taken process was NOT recycled
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2, "taken process must not be reused from idle");
}
