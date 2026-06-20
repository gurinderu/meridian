use meridian::rate_limit::RateLimitStore;
use serde_json::json;

#[test]
fn records_by_bucket_last_write_wins_and_filters_default() {
    let s = RateLimitStore::new();
    s.record(&json!({"status":"allowed","rateLimitType":"five_hour","utilization":0.2}));
    s.record(&json!({"status":"allowed_warning","rateLimitType":"five_hour","utilization":0.9})); // overwrites
    s.record(&json!({"status":"allowed","rateLimitType":"seven_day"}));
    s.record(&json!({"status":"allowed"})); // no rateLimitType -> "default" bucket, filtered out

    let all = s.get_all();
    assert_eq!(s.entry_count(), 2, "default bucket excluded from count");
    assert_eq!(all.len(), 2, "default bucket excluded from get_all");
    let five = all.iter().find(|b| b["type"] == "five_hour").unwrap();
    assert_eq!(five["status"], "allowed_warning"); // last write won
    assert_eq!(five["utilization"], 0.9);
    // normalized fields present with null fallback
    let seven = all.iter().find(|b| b["type"] == "seven_day").unwrap();
    assert_eq!(seven["utilization"], serde_json::Value::Null);
    assert_eq!(seven["isUsingOverage"], false);
}

#[test]
fn record_ignores_non_objects_and_clear_empties() {
    let s = RateLimitStore::new();
    s.record(&json!("not an object"));
    s.record(&json!(null));
    assert_eq!(s.entry_count(), 0);
    s.record(&json!({"rateLimitType":"five_hour","status":"allowed"}));
    assert_eq!(s.entry_count(), 1);
    s.clear();
    assert_eq!(s.entry_count(), 0);
}
