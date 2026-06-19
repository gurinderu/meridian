use meridian::error::ProxyError;

#[test]
fn status_codes_map_per_kind() {
    assert_eq!(ProxyError::BadRequest("x".into()).status(), 400);
    assert_eq!(ProxyError::Upstream("x".into()).status(), 502);
    assert_eq!(ProxyError::Internal("x".into()).status(), 500);
}
