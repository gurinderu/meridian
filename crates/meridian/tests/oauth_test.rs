use meridian::oauth::*;

#[test]
fn base64url_no_padding_and_urlsafe() {
    assert_eq!(base64url(b"abc"), "YWJj");          // no padding needed
    assert_eq!(base64url(b"ab"), "YWI");            // padding stripped
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");    // - and _ (not + /)
}

#[test]
fn pkce_challenge_matches_rfc7636_vector() {
    // RFC 7636 Appendix B
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(code_challenge_for(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
fn authorize_url_has_required_params() {
    let u = build_authorize_url("CHAL", "STATE");
    assert!(u.starts_with("https://claude.com/cai/oauth/authorize?"));
    for needle in ["code=true","client_id=9d1c250a","response_type=code",
                   "code_challenge=CHAL","code_challenge_method=S256","state=STATE"] {
        assert!(u.contains(needle), "missing {needle} in {u}");
    }
    assert!(u.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"));
    assert!(u.contains("scope=org%3Acreate_api_key")); // space-joined, url-encoded
}

#[test]
fn parse_code_from_raw_url_and_bare() {
    // pasted callback URL
    let p = parse_authorization_code("https://x/callback?code=AUTHCODE&state=ST").unwrap();
    assert_eq!(p.code, "AUTHCODE");
    assert_eq!(p.state.as_deref(), Some("ST"));
    // bare "code#state" form
    let p2 = parse_authorization_code("RAWCODE#ST2").unwrap();
    assert_eq!(p2.code, "RAWCODE");
    assert_eq!(p2.state.as_deref(), Some("ST2"));
    // bare code only
    let p3 = parse_authorization_code("JUSTCODE").unwrap();
    assert_eq!(p3.code, "JUSTCODE");
    // empty -> None
    assert!(parse_authorization_code("   ").is_none());
}

#[test]
fn new_session_is_well_formed() {
    let s = new_oauth_session().unwrap();
    assert!(!s.code_verifier.is_empty() && !s.state.is_empty());
    assert!(s.authorize_url.contains(&format!("state={}", s.state)));
    // challenge in the URL is derived from the verifier
    assert!(s.authorize_url.contains(&format!("code_challenge={}", code_challenge_for(&s.code_verifier))));
}

#[test]
fn build_credentials_maps_tokens() {
    let tok = serde_json::json!({"access_token":"AC","refresh_token":"RF","expires_in":3600,"scope":"a b"});
    let c = build_credentials_file(&tok, 1_000_000).unwrap();
    assert_eq!(c.claude_ai_oauth.access_token, "AC");
    assert_eq!(c.claude_ai_oauth.refresh_token, "RF");
    assert_eq!(c.claude_ai_oauth.expires_at, 1_000_000 + 3600*1000);
    // missing access/refresh -> None
    assert!(build_credentials_file(&serde_json::json!({"access_token":"x"}), 0).is_none());
}

#[test]
fn auth_status_parsing() {
    use meridian::oauth::auth_status_logged_in;
    assert!(auth_status_logged_in(r#"{"loggedIn":true,"email":"a@b.c"}"#));
    assert!(!auth_status_logged_in(r#"{"loggedIn":false}"#));
    assert!(!auth_status_logged_in("not json"));
}
