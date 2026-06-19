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
    IsolationKey { profile_id: p.into(), cwd: "/w".into(), options_hash: 0, resume: None }
}

#[tokio::test]
async fn reuses_warm_process_for_same_key() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 4);
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 1);
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
async fn discarded_lease_frees_slot_and_is_not_reused() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone(), fail: false }, 4);
    {
        let mut l = pool.acquire(&key("a")).await.unwrap().unwrap();
        l.discard();
    }
    assert_eq!(pool.live_count(), 0, "discard frees the cap slot");
    { let _l = pool.acquire(&key("a")).await.unwrap().unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2, "a discarded process must not be reused");
}
