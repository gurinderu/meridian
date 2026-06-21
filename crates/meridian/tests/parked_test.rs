use meridian::parked::ParkedStore;

#[test]
fn park_take_roundtrip_keyed_by_profile_and_session() {
    let s: ParkedStore<u32> = ParkedStore::new();
    assert!(s.park("p1".into(), "s1".into(), 100, 8).is_empty());
    // wrong profile / wrong session -> miss
    assert_eq!(s.take("p2", "s1"), None);
    assert_eq!(s.take("p1", "s2"), None);
    // exact key -> hit, and it's removed
    assert_eq!(s.take("p1", "s1"), Some(100));
    assert_eq!(s.take("p1", "s1"), None);
    assert_eq!(s.len(), 0);
}

#[test]
fn park_over_cap_evicts_lru() {
    let s: ParkedStore<u32> = ParkedStore::new();
    // cap = 2; insert 3 distinct keys -> the oldest is evicted and returned.
    assert!(s.park("p".into(), "a".into(), 1, 2).is_empty());
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(s.park("p".into(), "b".into(), 2, 2).is_empty());
    std::thread::sleep(std::time::Duration::from_millis(5));
    let evicted = s.park("p".into(), "c".into(), 3, 2);
    assert_eq!(evicted, vec![1], "the LRU entry (a=1) is evicted and returned");
    assert_eq!(s.len(), 2);
    assert_eq!(s.take("p", "a"), None);
    assert_eq!(s.take("p", "b"), Some(2));
    assert_eq!(s.take("p", "c"), Some(3));
}

#[test]
fn reap_returns_timed_out_entries() {
    let s: ParkedStore<u32> = ParkedStore::new();
    s.park("p".into(), "s".into(), 7, 8);
    std::thread::sleep(std::time::Duration::from_millis(20));
    // ttl shorter than the age -> reaped
    let reaped = s.reap(std::time::Duration::from_millis(10));
    assert_eq!(reaped, vec![7]);
    assert_eq!(s.len(), 0);
    // nothing left to reap
    assert!(s.reap(std::time::Duration::from_millis(0)).is_empty());
}

#[test]
fn same_key_repark_returns_displaced_proc() {
    let s: ParkedStore<u32> = ParkedStore::new();
    assert!(s.park("p".into(), "s".into(), 1, 8).is_empty());
    // re-parking the same (profile,session) must return the old proc for shutdown
    let displaced = s.park("p".into(), "s".into(), 2, 8);
    assert_eq!(displaced, vec![1], "old proc returned for graceful shutdown");
    assert_eq!(s.len(), 1);
    assert_eq!(s.take("p", "s"), Some(2));
}
