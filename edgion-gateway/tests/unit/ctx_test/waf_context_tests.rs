use super::*;
use edgion_resources::resources::edgion_plugins::{WafConfig, WafMode};
use edgion_waf::seclang::{RequestHead, RuleEngineMode, RuntimeLimits, Transaction};

fn request_headers_outcome(rules: &str, uri: &'static [u8]) -> edgion_waf::seclang::PhaseOutcome {
    let config = WafConfig {
        mode: WafMode::On,
        ..crate::plugins::http::waf::fixture::config(rules)
    };
    let generation = Arc::new(config.compile_generation().expect("test WAF compiles"));
    let request = RequestHead::new(
        b"GET /audit HTTP/1.1",
        b"GET",
        uri,
        uri,
        b"HTTP/1.1",
        b"192.0.2.1",
        b"request-1",
        &[],
    );
    let mut transaction = Transaction::new_shared_with_modes(
        generation,
        request,
        RuntimeLimits::default(),
        edgion_waf::seclang::EvaluationMode::Accelerated,
        RuleEngineMode::On,
    )
    .unwrap();
    transaction.execute_request_headers().unwrap()
}

#[test]
fn winner_selection_and_phase_claims_are_exact_once() {
    let selected = Arc::new(());
    let other = Arc::new(());
    let mut ctx = EdgionHttpContext::new();

    assert!(ctx.waf.is_none());
    assert!(ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&selected),
        WafLogSource::Route,
        10,
        WafLogMode::On,
    ))));
    assert!(!ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&other),
        WafLogSource::Global,
        20,
        WafLogMode::DetectionOnly,
    ))));
    assert_eq!(ctx.claim_waf_phase(&other, WafPhase::Request), WafPhaseClaim::Skip);
    for phase in [WafPhase::Request, WafPhase::ResponseHeaders, WafPhase::ResponseComplete] {
        assert_eq!(ctx.claim_waf_phase(&selected, phase), WafPhaseClaim::Execute);
        assert_eq!(ctx.claim_waf_phase(&selected, phase), WafPhaseClaim::Skip);
    }
}

#[test]
fn no_winner_keeps_the_unified_waf_state_unallocated() {
    let owner = Arc::new(());
    let mut ctx = EdgionHttpContext::new();

    assert!(ctx.select_waf(None));
    assert!(!ctx.select_waf(None));
    assert!(!ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&owner),
        WafLogSource::Route,
        0,
        WafLogMode::On,
    ))));
    assert!(ctx.waf.is_none());
    assert_eq!(ctx.claim_waf_phase(&owner, WafPhase::Request), WafPhaseClaim::Pending);
    assert_eq!(
        std::mem::size_of::<Option<Box<WafRequestContext>>>(),
        std::mem::size_of::<usize>()
    );
}

#[test]
fn selected_waf_allocates_one_request_owned_box_and_no_winner_allocates_none() {
    let owner = Arc::new(());
    let (selected, selected_allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&owner),
            WafLogSource::Route,
            0,
            WafLogMode::On,
        ))));
        ctx
    });
    assert!(selected.waf_access_log().is_some());
    assert_eq!(selected_allocations.calls, 1);
    assert_eq!(
        selected_allocations.requested_bytes,
        std::mem::size_of::<WafRequestContext>()
    );

    let (unselected, unselected_allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.select_waf(None));
        ctx
    });
    assert!(unselected.waf_access_log().is_none());
    assert_eq!(unselected_allocations.calls, 0);
    assert_eq!(unselected_allocations.requested_bytes, 0);
}

#[test]
fn access_summary_is_bounded_and_retains_intervention_outside_the_finding_cap() {
    let mut rules = String::new();
    for id in 1..=20 {
        rules.push_str(&format!(
            "SecRule REQUEST_URI \"@contains /audit\" \"id:{id},phase:1,pass,log,auditlog\"\n"
        ));
    }
    rules.push_str("SecRule REQUEST_URI \"@contains /audit\" \"id:999,phase:1,deny,status:409,log,auditlog\"\n");
    let outcome = request_headers_outcome(&rules, b"/audit-private-input");
    let mut access = WafAccessLog::new(WafLogSource::Route, 17, WafLogMode::On);
    access.record_phase(WafLogPhase::RequestHeaders, 23, &outcome);

    let value = serde_json::to_value(&access).unwrap();
    assert_eq!(value["source"], "route");
    assert_eq!(value["priority"], 17);
    assert_eq!(value["mode"], "on");
    assert_eq!(value["outcome"], "blocked");
    assert_eq!(value["findings"].as_array().unwrap().len(), MAX_WAF_ACCESS_LOG_FINDINGS);
    assert_eq!(value["findings_omitted"], 5);
    assert_eq!(value["phases"]["request_headers"]["findings"], 21);
    assert_eq!(value["phases"]["request_headers"]["evaluated_rules"], 21);
    assert_eq!(value["intervention"]["rule_id"], 999);
    assert_eq!(value["intervention"]["status"], 409);
    access.record_phase(WafLogPhase::RequestHeaders, 99, &outcome);
    assert_eq!(serde_json::to_value(&access).unwrap(), value);
    let json = value.to_string();
    assert!(!json.contains("audit-private-input"));
}

#[test]
fn nolog_finding_is_excluded_and_first_terminal_error_is_stable() {
    let outcome = request_headers_outcome(
        concat!(
            "SecRule REQUEST_URI \"@contains /audit\" \"id:1,phase:1,pass,nolog,noauditlog\"\n",
            "SecRule REQUEST_URI \"@contains /audit\" \"id:2,phase:1,pass,log\"\n",
        ),
        b"/audit",
    );
    let mut access = WafAccessLog::new(WafLogSource::Gateway, 3, WafLogMode::DetectionOnly);
    access.record_phase(WafLogPhase::RequestHeaders, 7, &outcome);
    access.record_error(WafLogPhase::RequestBody, WafErrorCode::BodyLimit, 413);
    access.record_error(WafLogPhase::Logging, WafErrorCode::RuntimeInvariant, 500);

    let value = serde_json::to_value(&access).unwrap();
    let findings = value["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(value["phases"]["request_headers"]["findings"], 1);
    assert_eq!(findings[0]["rule_id"], 2);
    assert_eq!(value["terminal_error"]["phase"], "request_body");
    assert_eq!(value["terminal_error"]["code"], "body_limit");
    assert_eq!(value["terminal_error"]["status"], 413);
}

#[test]
fn finalization_distinguishes_not_run_incomplete_and_allowed() {
    // The rule set must be non-empty to compile; `/clean` simply does not match it.
    let clean = request_headers_outcome(
        "SecRule REQUEST_URI \"@contains /detected\" \"id:1,phase:1,pass,log\"\n",
        b"/clean",
    );
    let detected = request_headers_outcome(
        "SecRule REQUEST_URI \"@contains /detected\" \"id:1,phase:1,pass,log\"\n",
        b"/detected",
    );

    let mut not_run = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
    not_run.finalize();
    assert_eq!(serde_json::to_value(&not_run).unwrap()["outcome"], "not_run");

    let mut incomplete = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
    incomplete.record_phase(WafLogPhase::RequestHeaders, 1, &clean);
    incomplete.finalize();
    assert_eq!(serde_json::to_value(&incomplete).unwrap()["outcome"], "incomplete");

    let mut incomplete_after_finding = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
    incomplete_after_finding.record_phase(WafLogPhase::RequestHeaders, 1, &detected);
    incomplete_after_finding.finalize();
    assert_eq!(
        serde_json::to_value(&incomplete_after_finding).unwrap()["outcome"],
        "incomplete"
    );

    let mut allowed = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::On);
    allowed.record_phase(WafLogPhase::Logging, 1, &clean);
    allowed.finalize();
    assert_eq!(serde_json::to_value(&allowed).unwrap()["outcome"], "allowed");

    let mut fully_detected = WafAccessLog::new(WafLogSource::Global, 0, WafLogMode::DetectionOnly);
    fully_detected.record_phase(WafLogPhase::Logging, 1, &detected);
    fully_detected.finalize();
    assert_eq!(serde_json::to_value(&fully_detected).unwrap()["outcome"], "detected");
}

#[test]
fn format_limit_is_owned_by_selected_waf_and_outlives_transaction() {
    let owner = Arc::new(());
    let other = Arc::new(());
    let mut ctx = EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = 32;
    assert!(!ctx.install_waf_body_hard_limit(&owner, 8));
    assert!(ctx.select_waf(Some(WafSelection::new(
        Arc::clone(&owner),
        WafLogSource::Route,
        0,
        WafLogMode::DetectionOnly
    ))));
    assert!(!ctx.install_waf_body_hard_limit(&other, 4));
    assert!(!ctx.install_waf_body_hard_limit(&owner, 0));
    assert!(ctx.install_waf_body_hard_limit(&owner, 8));
    assert!(!ctx.install_waf_body_hard_limit(&owner, 64));
    assert_eq!(ctx.request_body_hard_limit(), 8);
    assert!(ctx.waf.as_mut().unwrap().take(&owner).is_none());
    assert_eq!(ctx.request_body_hard_limit(), 8);
    assert!(!ctx.has_request_body_plugins());
    ctx.request_body_config.max_body_size_bytes = 4;
    assert_eq!(ctx.request_body_hard_limit(), 4);
}

#[test]
fn inspection_stop_claims_remaining_phases_and_keeps_hard_admission() {
    for storage in [false, true] {
        let owner = Arc::new(());
        let mut ctx = EdgionHttpContext::new();
        assert!(ctx.select_waf(Some(WafSelection::new(
            Arc::clone(&owner),
            WafLogSource::Route,
            0,
            WafLogMode::On
        ))));
        assert!(ctx.install_waf_body_hard_limit(&owner, 64));
        assert!(!ctx.stop_waf_inspection(&Arc::new(()), 32, storage));
        assert!(ctx.stop_waf_inspection(&owner, 32, storage));
        assert_eq!(ctx.request_body_hard_limit(), 64);
        for phase in [WafPhase::Request, WafPhase::ResponseHeaders, WafPhase::ResponseComplete] {
            assert_eq!(ctx.claim_waf_phase(&owner, phase), WafPhaseClaim::Skip);
        }
        let summary = serde_json::to_value(&ctx.waf.as_ref().unwrap().access_log).unwrap();
        assert_eq!(summary["outcome"], "incomplete");
        assert_eq!(summary["inspection_stop"]["admitted_bytes"], 32);
        assert_eq!(
            summary["inspection_stop"]["reason"],
            if storage { "storage" } else { "inspection" }
        );
    }
}
