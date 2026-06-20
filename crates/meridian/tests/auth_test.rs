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
fn constant_time_eq_rejects_nul_padded_short_input() {
    // a provided value shorter than the secret must never compare equal, even
    // if the missing positions would XOR to zero against a (hypothetical) NUL
    // secret byte — the length fold guards this.
    assert!(!constant_time_eq(b"sec", b"secret"));
    assert!(!constant_time_eq(b"sec\x00\x00\x00", b"secret"));
    assert!(!constant_time_eq(b"", b"secret"));
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
