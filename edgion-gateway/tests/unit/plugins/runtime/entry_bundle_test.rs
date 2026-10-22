use super::*;
use serde_json::json;

fn plugin(value: serde_json::Value) -> EdgionPlugin {
    serde_json::from_value(value).unwrap()
}

#[test]
fn outcome_slots_compact_referenced_producers_and_freeze_source_bindings() {
    let entries: Vec<RequestFilterEntry> = serde_json::from_value(json!([
        {"type": "RequestHeaderModifier", "config": {}},
        {"type": "RateLimitLocal", "alias": "unused", "config": {"rate": 100}},
        {"type": "RateLimitLocal", "alias": "first", "config": {"rate": 100}},
        {"type": "RateLimitLocal", "alias": "disabled", "enable": false, "config": {"rate": 100}},
        {"type": "RateLimitLocal", "config": {"rate": 100}},
        {"type": "RateLimitLocal", "alias": "second", "config": {"rate": 100}},
        {"type": "RateLimitRedis", "conditions": {"run": {"allOf": [
            {"type": "rateLimitExceeded", "pluginAlias": "first"},
            {"type": "rateLimitExceeded", "pluginAlias": "second"},
            {"type": "rateLimitExceeded", "pluginAlias": "disabled"}
        ]}}, "config": {"redisRef": "apps/unused", "quotas": []}}
    ]))
    .unwrap();
    let mut cache = EntryCache::default();
    let expected = HashMap::from([("first".to_owned(), 7), ("second".to_owned(), 8)]);
    assert_eq!(cache.prepare_request_outcomes(&entries, 7), &expected);
    assert_eq!(cache.request_source_aliases()[2].as_deref(), Some("first"));
    assert_eq!(cache.request_source_aliases()[5].as_deref(), Some("second"));
    // A later policy view/base must not rebind shared producer instances.
    assert_eq!(cache.prepare_request_outcomes(&[], 100), &expected);
    assert_eq!(cache.request_source_aliases().len(), entries.len());
}

#[test]
fn response_cache_certificate_requires_exact_hidden_header_auth_profile() {
    let valid = json!({"type": "KeyAuth", "config": {
        "keySources": [{"type": "header", "name": "X-Cache-Key"}], "hideCredentials": true,
        "upstreamHeaderFields": []
    }});
    let good = CacheCapabilities::for_constructed(&plugin(valid.clone()), EntryStage::Request);
    assert!(good.safe);
    assert_eq!(good.credential_headers, ["x-cache-key"]);
    for field in ["hideCredentials", "upstreamHeaderFields", "anonymous", "keySources"] {
        let mut bad = valid.clone();
        bad["config"][field] = match field {
            "hideCredentials" => json!(false),
            "upstreamHeaderFields" => json!(["username"]),
            "anonymous" => json!("anonymous"),
            _ => json!([{ "type": "query", "name": "key" }]),
        };
        assert!(!CacheCapabilities::for_constructed(&plugin(bad), EntryStage::Request).safe);
    }
}

#[test]
fn response_cache_response_companions_and_unknown_effects_stay_unsafe() {
    let dsl = plugin(json!({"type": "Dsl", "config": {"name": "request-only", "source": "let a = 1"}}));
    assert!(CacheCapabilities::for_constructed(&dsl, EntryStage::Request).safe);
    assert!(!CacheCapabilities::for_constructed(&dsl, EntryStage::ResponseFilter).safe);
    let safe =
        plugin(json!({"type": "ResponseHeaderModifier", "config": {"set": [{"name":"x-public", "value":"yes"}]}}));
    assert!(CacheCapabilities::for_constructed(&safe, EntryStage::ResponseFilter).safe);
    assert!(!CacheCapabilities::for_constructed(&safe, EntryStage::ResponseBody).safe);
    assert!(!CacheCapabilities::for_constructed(&EdgionPlugin::Unsupported, EntryStage::Request).safe);
    let mut combined = CacheCapabilities::default();
    combined.combine(&CacheCapabilities::for_constructed(&dsl, EntryStage::ResponseFilter));
    combined.combine(&CacheCapabilities::for_constructed(&safe, EntryStage::ResponseFilter));
    assert!(!combined.safe, "safe sibling cannot erase an unsafe companion");
}
