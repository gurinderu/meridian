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
