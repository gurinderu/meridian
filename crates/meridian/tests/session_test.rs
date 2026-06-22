use serde_json::json;
use meridian::session::{fingerprint, SessionStore};

#[test]
fn fingerprint_is_stable_and_content_sensitive() {
    let a = vec![json!({"role":"user","content":"hi"}), json!({"role":"assistant","content":"yo"})];
    let b = vec![json!({"role":"user","content":"hi"}), json!({"role":"assistant","content":"yo"})];
    let c = vec![json!({"role":"user","content":"hi"}), json!({"role":"assistant","content":"different"})];
    assert_eq!(fingerprint(&a), fingerprint(&b), "same content -> same key");
    assert_ne!(fingerprint(&a), fingerprint(&c), "different content -> different key");
}

#[test]
fn fingerprint_handles_block_array_content() {
    let s = vec![json!({"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]})];
    let plain = vec![json!({"role":"user","content":"a\nb"})];
    assert_eq!(fingerprint(&s), fingerprint(&plain), "block array text == joined string");
}

#[test]
fn store_round_trips() {
    let store = SessionStore::new();
    assert_eq!(store.get("k"), None);
    store.insert("k".into(), "sess-1".into());
    assert_eq!(store.get("k"), Some("sess-1".into()));
    store.insert("k".into(), "sess-2".into());
    assert_eq!(store.get("k"), Some("sess-2".into()), "insert overwrites");
}

#[test]
fn clear_evicts_all_sessions() {
    let store = SessionStore::new();
    let fp = fingerprint(&[]);
    store.insert(fp.clone(), "sess-1".into());
    assert!(store.get(&fp).is_some());
    store.clear();
    assert!(store.get(&fp).is_none());
}

#[test]
fn lru_evicts_oldest_over_cap() {
    use meridian::session::SessionStore;
    let s = SessionStore::with_cap(2);
    s.insert("a".into(), "sa".into());
    s.insert("b".into(), "sb".into());
    s.insert("c".into(), "sc".into()); // over cap -> evict oldest ("a")
    assert_eq!(s.len_for_test(), 2);
    assert_eq!(s.get("a"), None, "oldest entry evicted");
    assert_eq!(s.get("b").as_deref(), Some("sb"));
    assert_eq!(s.get("c").as_deref(), Some("sc"));
    // re-inserting an existing key refreshes its recency (largest seq), so the
    // next overflow evicts the now-oldest ("b"), not "c".
    s.insert("c".into(), "sc2".into());
    s.insert("d".into(), "sd".into()); // evict oldest among {b,c} -> "b"
    assert_eq!(s.get("b"), None);
    assert_eq!(s.get("c").as_deref(), Some("sc2"));
    assert_eq!(s.get("d").as_deref(), Some("sd"));
}
