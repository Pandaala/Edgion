use super::*;
use crate::ctx::{WafLogMode, WafLogSource, WafSelection};
use crate::geoip::GeoInfo;
use crate::request_trace_id::CompiledRequestIdRule;
use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use http::{HeaderMap, HeaderValue};
use std::sync::Arc;

#[test]
fn empty_diagnostics_and_full_marker_are_omitted() {
    let json = AccessLogEntry::from_context(&EdgionHttpContext::new()).to_json();
    assert!(!json.contains(r#""diagnostics""#), "got: {json}");
    assert!(!json.contains(r#""diagnostics_full""#), "got: {json}");
}

#[test]
fn request_info_has_trace_id_string_and_custom_request_ids() {
    let mut ctx = EdgionHttpContext::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        "traceparent",
        HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
    );
    ctx.request_info.trace_id.initialize(&headers);
    let rule = serde_json::from_value(serde_json::json!({"header":"x-order-id","source":{"type":"traceId"}})).unwrap();
    let rule = CompiledRequestIdRule::new(&rule).unwrap();
    ctx.request_info.trace_id.apply_rule(
        &rule,
        None,
        Some(HeaderValue::from_static("4bf92f3577b34da6a3ce929d0e0e4736")),
    );

    let entry: serde_json::Value = serde_json::from_str(&AccessLogEntry::from_context(&ctx).to_json()).unwrap();
    assert!(entry.get("trace_id").is_none());
    assert_eq!(entry["request_info"]["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(
        entry["request_info"]["request_ids"]["x-order-id"],
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert!(entry["request_info"]["request_ids"].get("x-trace-id").is_none());
}

#[test]
fn access_log_read_does_not_create_protocol_scenario_state() {
    let ctx = EdgionHttpContext::new();
    assert!(!ctx.request_info.has_tls_info());
    assert!(!ctx.request_info.has_grpc_info());

    let _entry = AccessLogEntry::from_context(&ctx);

    assert!(!ctx.request_info.has_tls_info());
    assert!(!ctx.request_info.has_grpc_info());
}

#[test]
fn waf_summary_is_attached_only_by_terminal_logging() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.select_waf(Some(WafSelection::new(
        Arc::new(()),
        WafLogSource::Route,
        9,
        WafLogMode::DetectionOnly,
    ))));

    let intermediate = AccessLogEntry::from_context(&ctx).to_json();
    assert!(!intermediate.contains("\"waf\""));

    ctx.finalize_waf_for_logging();
    let mut terminal = AccessLogEntry::from_context(&ctx);
    terminal.waf = ctx.waf_access_log();
    let value: serde_json::Value = serde_json::from_str(&terminal.to_json()).unwrap();
    assert_eq!(value["waf"]["source"], "route");
    assert_eq!(value["waf"]["priority"], 9);
    assert_eq!(value["waf"]["mode"], "detection_only");
    assert_eq!(value["waf"]["outcome"], "not_run");
}

#[test]
fn diagnostic_overflow_emits_the_full_marker_and_only_kind_and_code() {
    let mut ctx = EdgionHttpContext::new();
    for _ in 0..=crate::observe::request_diagnostics::MAX_REQUEST_DIAGNOSTICS {
        ctx.record_error(EdgionErrorCode::Unknown);
    }

    let value: serde_json::Value = serde_json::from_str(&AccessLogEntry::from_context(&ctx).to_json()).unwrap();
    let diagnostics = value["diagnostics"].as_array().unwrap();
    assert_eq!(
        diagnostics.len(),
        crate::observe::request_diagnostics::MAX_REQUEST_DIAGNOSTICS
    );
    assert_eq!(value["diagnostics_full"], true);
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.as_object().is_some_and(|object| {
            object.len() == 2
                && object.get("kind") == Some(&serde_json::json!("error"))
                && object.get("code") == Some(&serde_json::json!("Unknown"))
        })
    }));
}

#[test]
fn serialization_fallback_uses_the_diagnostic_schema() {
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&access_log_fallback(123)).unwrap(),
        serde_json::json!({
            "ts": 123,
            "diagnostics": [{"kind": "error", "code": "AccessLogSerializeFailed"}]
        })
    );
}

#[test]
fn geo_block_is_omitted_when_no_geo_lookup_ran() {
    let ctx = EdgionHttpContext::default();
    let entry = AccessLogEntry::from_context(&ctx);
    assert!(!entry.to_json().contains("\"geo\""));
}

/// A slot nobody resolved is ABSENT, which is what distinguishes it from a
/// slot that was resolved and found nothing. Collapsing the two would make
/// "no plugin asked" indistinguishable from "asked, database had no record".
#[test]
fn unresolved_slot_is_absent_while_resolved_slots_report_their_status() {
    let block =
        GeoLogBlock::from_slots(Some(&GeoLookup::NotFound), None).expect("one resolved slot must produce a block");
    let json = serde_json::to_string(&block).unwrap();
    assert!(
        json.contains("not_found"),
        "resolved slot must report its status: {json}"
    );
    assert!(
        !json.contains("direct_peer"),
        "unresolved slot must be absent entirely: {json}"
    );
}

/// Unavailable (no database at all), NotFound (database loaded, no
/// record), and Failed (loaded database, decode/panic) stay distinct.
#[test]
fn unavailable_not_found_and_failed_remain_distinguishable() {
    let block = GeoLogBlock::from_slots(Some(&GeoLookup::Unavailable), Some(&GeoLookup::NotFound))
        .expect("resolved slots must produce a block");
    let json = serde_json::to_string(&block).unwrap();
    assert!(json.contains("unavailable"));
    assert!(json.contains("not_found"));

    let failed = GeoLogBlock::from_slots(Some(&GeoLookup::Failed), None).expect("failed slot must produce a block");
    let failed_json = serde_json::to_string(&failed).unwrap();
    assert!(
        failed_json.contains("lookup_failed"),
        "Failed must serialize as lookup_failed: {failed_json}"
    );
    assert!(!failed_json.contains("unavailable"));
    assert!(!failed_json.contains("not_found"));
    assert!(!failed_json.contains("country"));
}

/// Building an access-log entry must never force a lookup, so a request
/// whose plugins never consulted geo emits no `geo` key at all.
#[test]
fn no_resolved_slot_yields_no_block() {
    assert!(GeoLogBlock::from_slots(None, None).is_none());
    assert!(GeoLogBlock::from_slots(None, None).is_none());
}

#[test]
fn coordinates_are_emitted_masked_when_the_record_carries_them() {
    let with_coords = std::sync::Arc::new(GeoInfo {
        country_iso: Some("CN".into()),
        latitude: Some(39.9),
        longitude: Some(116.4),
        ..Default::default()
    });
    let block = GeoLogBlock::from_slots(Some(&GeoLookup::Found(with_coords)), None).unwrap();
    let json = serde_json::to_string(&block).unwrap();
    assert!(
        json.contains("coord"),
        "a record carrying coordinates must emit coord: {json}"
    );
    // Whether the value renders in clear is governed by the process-global
    // `SENSITIVE_MODE`, which is Off in unit tests — so this asserts the field
    // is emitted, not its masked rendering. The masking itself belongs to the
    // shared `masked` serializer and is covered where that mode is exercised.

    // A record with no coordinates emits no `coord` key at all — there is no
    // switch to turn them off, so absence must mean "the record had none".
    let without = std::sync::Arc::new(GeoInfo {
        country_iso: Some("CN".into()),
        ..Default::default()
    });
    let block = GeoLogBlock::from_slots(Some(&GeoLookup::Found(without)), None).unwrap();
    assert!(!serde_json::to_string(&block).unwrap().contains("coord"));
}

#[test]
fn access_log_includes_acme_information_diagnostic() {
    let mut ctx = EdgionHttpContext::new();
    ctx.record_info(EdgionInfoCode::AcmeChallengeServed);

    let json = AccessLogEntry::from_context(&ctx).to_json();

    assert!(
        json.contains(r#""diagnostics":[{"kind":"info","code":"AcmeChallengeServed"}]"#),
        "access log must include ACME local response status, got: {json}"
    );
    assert!(
        !json.contains(r#""kind":"error""#),
        "ACME local response should not be reported as an error, got: {json}"
    );
}

#[test]
fn ctx_map_never_reaches_the_log_without_a_declaration() {
    // The annotation allowlist is gone: ctx vars reach the log only through
    // an AccessLogExtern `from: ctx` declaration, which populates `ext`.
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("user_tier".to_string(), "gold".to_string());
    ctx.set_ctx_var("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());

    let json = AccessLogEntry::from_context(&ctx).to_json();

    assert!(!json.contains("user_tier") && !json.contains("gold"), "got: {json}");
    assert!(!json.contains("jwt_claims") && !json.contains("alice"), "got: {json}");
    assert!(!json.contains(r#""ext""#), "empty ext must be omitted, got: {json}");
}

#[test]
fn access_log_projection_does_not_instantiate_plugin_aux() {
    let ctx = EdgionHttpContext::new();
    assert!(!ctx.has_plugin_aux());
    assert!(!ctx.has_response_processing());

    let json = AccessLogEntry::from_context(&ctx).to_json();

    assert!(!ctx.has_plugin_aux());
    assert!(!ctx.has_response_processing());
    assert!(!json.contains(r#""stage_logs""#));
}

#[test]
fn upstream_authority_is_logged_only_when_present() {
    let mut ctx = EdgionHttpContext::new();
    assert!(
        !AccessLogEntry::from_context(&ctx)
            .to_json()
            .contains("upstream_authority"),
        "absent authority must be omitted from the access log"
    );

    ctx.upstream_authority = Some("tenant-a.sandbox.internal".to_string());
    let json = AccessLogEntry::from_context(&ctx).to_json();
    assert!(json.contains("tenant-a.sandbox.internal"), "got: {json}");
}
