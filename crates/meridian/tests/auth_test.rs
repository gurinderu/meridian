use meridian::auth::{constant_time_eq, extract_key, is_authorized};

#[test]
fn constant_time_eq_basic() {
    assert!(constant_time_eq(b"secret", b"secret"));
    assert!(!constant_time_eq(b"secret", b"secreT"));
    assert!(!constant_time_eq(b"short", b"longer-secret"));
    assert!(!constant_time_eq(b"longer-provided", b"key"));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn extract_prefers_x_api_key_then_bearer() {
    assert_eq!(extract_key(Some("k1"), Some("Bearer k2")), Some("k1"));
    assert_eq!(extract_key(None, Some("Bearer k2")), Some("k2"));
    assert_eq!(extract_key(Some(""), Some("Bearer k2")), Some("k2")); // empty x-api-key falls through
    assert_eq!(extract_key(None, Some("Basic abc")), None);           // only Bearer
    assert_eq!(extract_key(None, None), None);
}

#[test]
fn is_authorized_open_when_unconfigured() {
    assert!(is_authorized(None, None, None));
    assert!(is_authorized(Some(""), None, None)); // empty configured = disabled
}

#[test]
fn is_authorized_requires_match_when_configured() {
    assert!(is_authorized(Some("secret"), Some("secret"), None));
    assert!(is_authorized(Some("secret"), None, Some("Bearer secret")));
    assert!(!is_authorized(Some("secret"), Some("wrong"), None));
    assert!(!is_authorized(Some("secret"), None, None));        // missing
    assert!(!is_authorized(Some("secret"), None, Some("secret"))); // not Bearer-prefixed
}
