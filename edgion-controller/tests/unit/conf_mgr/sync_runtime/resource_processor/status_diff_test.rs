use super::*;

#[test]
fn controller_observations_compare_identity_and_native_semantics() {
    use serde_json::json;
    for kind in ["EdgionPlugins", "EdgionStreamPlugins", "EdgionBackend", "LinkSys"] {
        let entry = |controller: &str| {
            json!({
                "controllerName": controller, "status": {"conditions": [{
                    "type": "ResolvedRefs", "status": "True", "reason": "ResolvedRefs",
                    "message": "Resolved", "observedGeneration": 1,
                    "lastTransitionTime": "2026-01-01T00:00:00Z"
                }]}
            })
        };
        let old = json!({"controllers": [entry("a"), entry("b")]});
        let mut new = json!({"controllers": [entry("b"), entry("a")]});
        new["controllers"][0]["status"]["conditions"][0]["lastTransitionTime"] = json!("2026-02-01T00:00:00Z");
        let changed = |value: &Value| status_semantically_changed(kind, &old.to_string(), &value.to_string(), None);
        assert!(!changed(&new), "{kind}: order and transition time are not changes");
        let mut changed_owner = new.clone();
        changed_owner["controllers"][0]["controllerName"] = json!("new/controller");
        assert!(changed(&changed_owner));
        let mut changed_condition = new.clone();
        changed_condition["controllers"][0]["status"]["conditions"][0]["status"] = json!("False");
        assert!(changed(&changed_condition));
        let duplicate = json!({"controllers": [entry("a"), entry("a")]});
        assert!(changed(&duplicate));
        assert!(changed(&json!({"conditions": []})));
        if kind == "LinkSys" {
            new["controllers"][0]["status"]["publishedGeneration"] = json!(2);
            assert!(changed(&new));
        }
    }
}

#[test]
fn policy_ancestors_share_canonical_identity_and_multiset_comparison() {
    use serde_json::json;
    for kind in ["BackendTLSPolicy", "EdgionBackendTrafficPolicy"] {
        let entry = json!({"ancestorRef": {"name": "gw"}, "controllerName": "ours/controller",
                "conditions": [{"type": "Accepted", "status": "True", "observedGeneration": 1}]});
        let status = |entries: Vec<Value>| json!({"ancestors": entries}).to_string();
        let changed = |old: &str, new: &str| status_semantically_changed(kind, old, new, Some("apps"));
        let old = status(vec![entry.clone()]);
        let mut equivalent = entry.clone();
        equivalent["ancestorRef"] = json!({"name": "gw", "namespace": "apps",
                "group": "gateway.networking.k8s.io", "kind": "Gateway"});
        equivalent["conditions"][0]["lastTransitionTime"] = json!("2026-01-01T00:00:00Z");
        assert!(!changed(&old, &status(vec![equivalent])));
        for (field, value) in [
            ("group", json!("")),
            ("kind", json!("Service")),
            ("namespace", json!("other")),
            ("name", json!("other")),
            ("sectionName", json!("https")),
            ("port", json!(443)),
        ] {
            let mut different = entry.clone();
            different["ancestorRef"][field] = value;
            assert!(changed(&old, &status(vec![different])), "{kind}: {field}");
        }
        let mut foreign = entry.clone();
        foreign["controllerName"] = json!("foreign/controller");
        assert!(changed(&old, &status(vec![foreign.clone()])));
        let mixed = status(vec![foreign.clone(), entry.clone()]);
        assert!(!changed(&mixed, &mixed));
        assert!(!changed(&mixed, &status(vec![entry.clone(), foreign])));
        let mut rejected = entry.clone();
        rejected["conditions"][0]["status"] = json!("False");
        assert!(changed(&old, &status(vec![rejected.clone()])));
        let duplicates = status(vec![entry.clone(), rejected.clone()]);
        assert!(!changed(&duplicates, &status(vec![rejected, entry.clone()])));
        assert!(changed(&duplicates, &status(vec![entry.clone(), entry.clone()])));
        assert!(changed(&old, &status(vec![entry.clone(), entry])));
        for malformed in [
            "null",
            "[]",
            "1",
            r#"{"ancestors":{}}"#,
            r#"{"ancestors":"invalid"}"#,
            r#"{"ancestors":[{"ancestorRef":{"name":1},"controllerName":"c"}]}"#,
            r#"{"ancestors":[{"ancestorRef":{"name":"gw"}}]}"#,
        ] {
            for other in [malformed, "{}", r#"{"ancestors":[]}"#] {
                assert!(changed(malformed, other), "{kind}: {malformed}");
                assert!(changed(other, malformed), "{kind}: {malformed}");
            }
        }
        assert!(!changed("{}", r#"{"ancestors":[]}"#));
        assert!(!changed(r#"{"ancestors":null}"#, r#"{"ancestors":[]}"#));
    }
}

#[test]
fn malformed_parent_status_containers_are_not_equal() {
    for malformed in ["null", "[]", "1", r#"{"parents":{}}"#, r#"{"parents":"invalid"}"#] {
        for other in [malformed, "{}", r#"{"parents":[]}"#] {
            assert!(status_semantically_changed("HTTPRoute", malformed, other, Some("apps")));
            assert!(status_semantically_changed("HTTPRoute", other, malformed, Some("apps")));
        }
    }
}

#[test]
fn parent_status_comparison_uses_canonical_owned_identity_for_every_family() {
    use serde_json::json;
    let entry = json!({"parentRef": {"name": "gw"}, "controllerName": "custom.example/controller",
            "conditions": [{"type": "Accepted", "status": "False", "reason": "Test", "message": "m",
                "observedGeneration": 1, "lastTransitionTime": "2026-01-01T00:00:00Z"}]});
    let old = json!({"parents": [entry.clone()]}).to_string();
    for kind in [
        "HTTPRoute",
        "GRPCRoute",
        "TCPRoute",
        "TLSRoute",
        "UDPRoute",
        "EdgionTls",
    ] {
        let changed = |entries: Vec<Value>| {
            status_semantically_changed(kind, &old, &json!({"parents": entries}).to_string(), Some("apps"))
        };
        let mut equivalent = entry.clone();
        equivalent["parentRef"] = json!({"name": "gw", "namespace": "apps",
                "kind": "Gateway", "group": "gateway.networking.k8s.io"});
        equivalent["conditions"][0]["lastTransitionTime"] = json!("2026-02-01T00:00:00Z");
        assert!(!changed(vec![equivalent]), "{kind}");
        for (field, value) in [
            ("group", json!("")),
            ("kind", json!("HTTPRoute")),
            ("namespace", json!("elsewhere")),
            ("name", json!("other")),
            ("sectionName", json!("https")),
            ("port", json!(443)),
        ] {
            let mut different = entry.clone();
            different["parentRef"][field] = value;
            assert!(changed(vec![different]), "{kind}: {field}");
        }
        let mut other_owner = entry.clone();
        other_owner["controllerName"] = json!("foreign.example/controller");
        assert!(changed(vec![other_owner]));
        let mut generation = entry.clone();
        generation["conditions"][0]["observedGeneration"] = json!(2);
        assert!(changed(vec![generation]));
        assert!(changed(vec![entry.clone(), entry.clone()]));
    }
}

#[test]
fn parent_status_comparison_consumes_duplicate_entries_once_and_ignores_order() {
    use serde_json::json;
    let accepted = json!({"parentRef": {"name": "gw"}, "controllerName": "custom/controller",
            "conditions": [{"type": "Accepted", "status": "True"}]});
    let mut rejected = accepted.clone();
    rejected["conditions"][0]["status"] = json!("False");
    let old = json!({"parents": [accepted.clone(), rejected.clone()]}).to_string();
    let reordered = json!({"parents": [rejected, accepted.clone()]}).to_string();
    assert!(!status_semantically_changed(
        "HTTPRoute",
        &old,
        &reordered,
        Some("apps")
    ));
    let duplicated = json!({"parents": [accepted.clone(), accepted]}).to_string();
    assert!(status_semantically_changed(
        "HTTPRoute",
        &old,
        &duplicated,
        Some("apps")
    ));
    let malformed = json!({"parents": [{"parentRef": {"name": 1}, "controllerName": "c"}]}).to_string();
    assert!(status_semantically_changed(
        "HTTPRoute",
        &malformed,
        &malformed,
        Some("apps")
    ));
}

fn acme_status(ltt: &str, cond_status: &str, phase: &str) -> String {
    format!(
        r#"{{"conditions":[{{"type":"Accepted","status":"{cond_status}","reason":"Accepted","message":"ok","lastTransitionTime":"{ltt}","observedGeneration":1}}],"phase":"{phase}"}}"#
    )
}

#[test]
fn acme_ltt_only_change_is_not_a_status_change() {
    let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
    let new = acme_status("2026-07-19T00:00:00Z", "True", "Ready");
    assert!(
        !status_semantically_changed("EdgionAcme", &old, &new, None),
        "a lastTransitionTime-only difference must not trigger a status write"
    );
}

#[test]
fn acme_condition_flip_is_a_status_change() {
    let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
    let new = acme_status("2026-01-01T00:00:00Z", "False", "Ready");
    assert!(status_semantically_changed("EdgionAcme", &old, &new, None));
}

#[test]
fn acme_non_condition_field_change_is_a_status_change() {
    let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
    let new = acme_status("2026-01-01T00:00:00Z", "True", "Renewing");
    assert!(status_semantically_changed("EdgionAcme", &old, &new, None));
}

#[test]
fn link_sys_ltt_only_change_is_not_a_status_change() {
    let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1"}"#;
    let new = old.replace("2026-01-01", "2026-02-02");
    assert!(
        !status_semantically_changed("LinkSys", old, &new, None),
        "a lastTransitionTime-only difference must not trigger a status write"
    );
}

#[test]
fn link_sys_credential_field_change_is_a_status_change() {
    let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
    let new = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1","desiredConsumerCount":1,"publishedConsumerCount":1,"conflictedConsumerCount":0}"#;
    assert!(
        status_semantically_changed("LinkSys", old, new, None),
        "credential lifecycle fields must trigger a status write"
    );
}

#[test]
fn link_sys_credential_field_clear_is_a_status_change() {
    let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1"}"#;
    let new = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
    assert!(
        status_semantically_changed("LinkSys", old, new, None),
        "clearing stale credential lifecycle fields must trigger a status write"
    );
}

/// `EdgionBackendTrafficPolicy` (GEP-2648 `status.ancestors[]`, like
/// `BackendTLSPolicy`) must dispatch through `attachment_statuses_equal`: a
/// `lastTransitionTime`-only change is not a semantic change, but a
/// `status` flip on a condition is.
#[test]
fn ebtp_uses_ancestors_comparison() {
    let old = r#"{"ancestors":[{"ancestorRef":{"name":"gw"},"controllerName":"c","conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"m","lastTransitionTime":"2026-01-01T00:00:00Z"}]}]}"#;
    let new_time_only = old.replace("2026-01-01", "2026-02-02");
    assert!(
        !status_semantically_changed("EdgionBackendTrafficPolicy", old, &new_time_only, None),
        "lastTransitionTime-only change must not trigger a status write"
    );
    let new_flipped = old.replace("\"status\":\"True\"", "\"status\":\"False\"");
    assert!(status_semantically_changed(
        "EdgionBackendTrafficPolicy",
        old,
        &new_flipped,
        None
    ));
}
