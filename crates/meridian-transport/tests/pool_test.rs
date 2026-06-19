use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use meridian_transport::pool::{IsolationKey, Pool, ProcessFactory};

#[derive(Default)]
struct CountingFactory { spawned: Arc<AtomicUsize> }
struct FakeProc { #[allow(dead_code)] id: usize }
impl ProcessFactory for CountingFactory {
    type Proc = FakeProc;
    fn spawn(&self, _k: &IsolationKey) -> FakeProc {
        FakeProc { id: self.spawned.fetch_add(1, Ordering::SeqCst) }
    }
}

fn key(p: &str) -> IsolationKey {
    IsolationKey { profile_id: p.into(), cwd: "/w".into(), options_hash: 0 }
}

#[test]
fn reuses_warm_process_for_same_key() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone() }, 4);
    { let _l = pool.acquire(&key("a")).unwrap(); }       // spawn #0, then returned warm
    { let _l = pool.acquire(&key("a")).unwrap(); }       // reuse warm, no new spawn
    assert_eq!(spawned.load(Ordering::SeqCst), 1);
}

#[test]
fn different_keys_do_not_share() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone() }, 4);
    { let _a = pool.acquire(&key("a")).unwrap(); }
    { let _b = pool.acquire(&key("b")).unwrap(); }
    assert_eq!(spawned.load(Ordering::SeqCst), 2);
}

#[test]
fn global_cap_blocks_when_all_leases_live() {
    let spawned = Arc::new(AtomicUsize::new(0));
    let pool = Pool::new(CountingFactory { spawned: spawned.clone() }, 1);
    let _l1 = pool.acquire(&key("a")).unwrap();
    assert!(pool.acquire(&key("b")).is_none(), "cap=1 must refuse a 2nd live lease");
    assert_eq!(pool.live_count(), 1);
}
