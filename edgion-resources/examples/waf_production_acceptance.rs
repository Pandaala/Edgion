//! Strict acceptance of the WAF configurations used by the Gateway wire scenario.

use std::{collections::BTreeSet, fs, path::Path};

use edgion_resources::resources::edgion_config_data::{WafPolicyData, WafRuleBundleData};
use edgion_resources::resources::edgion_plugins::plugin_configs::ResolvedWafBundle;
use edgion_resources::resources::edgion_plugins::plugin_configs::WafConfig;
use edgion_waf::seclang::{
    BodyProcessorSelection, EvaluationMode, PhaseOutcome, RequestBody, RequestHead, RequestHeader, ResponseHead,
    ResponseWindow, RuleEngineMode, RuntimeLimits, Transaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const REVIEWED_MANIFEST_SHA256: &str = "15751e2f15103ceff566df1ec4aa94eb7ee95b27574bdc1735a2677e0a1d24db";
const BASELINE_CASES: [(&str, &str, u16, Option<u64>); 9] = [
    ("initial-allow", "initial", 200, None),
    ("initial-header-deny", "initial", 403, Some(1001)),
    ("initial-body-deny", "initial", 406, Some(1002)),
    ("initial-response-deny", "initial", 403, Some(1003)),
    ("updated-allow", "updated", 200, None),
    ("updated-header-deny", "updated", 451, Some(2001)),
    ("updated-body-deny", "updated", 406, Some(2002)),
    ("detection-allow", "detection", 200, None),
    ("detection-match", "detection", 200, Some(2501)),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    profile: String,
    corpus_revision: String,
    profiles: Vec<Profile>,
    cases: Vec<Case>,
    exclusions: Vec<Exclusion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    path: String,
    sha256: String,
    policy: BundleFile,
    rule_ids: Vec<u64>,
    bundles: Vec<BundleFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleFile {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    profile: String,
    version: String,
    body: String,
    response_deny: bool,
    expected_status: u16,
    expected_rule_id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusion {
    case_id: String,
    reason: String,
    revisit_when: String,
}

#[derive(Deserialize)]
struct PluginDocument {
    spec: PluginSpec,
}

#[derive(Deserialize)]
struct PluginSpec {
    waf: WafConfig,
}

#[derive(Serialize)]
struct CaseResult<'a> {
    id: &'a str,
    profile: &'a str,
    expected_status: u16,
    actual_status: Option<u16>,
    expected_rule_id: Option<u64>,
    actual_rule_id: Option<u64>,
    passed: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct Report<'a> {
    schema: &'static str,
    profile: &'a str,
    corpus_revision: &'a str,
    manifest_sha256: String,
    total: usize,
    supported: usize,
    executed: usize,
    passed: usize,
    failed: usize,
    excluded: usize,
    false_positives: usize,
    false_negatives: usize,
    cases: Vec<CaseResult<'a>>,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn observe(config: &WafConfig, case: &Case) -> Result<(u16, Option<u64>), String> {
    let generation = config.compile_generation()?;
    let version = case.version.as_bytes();
    let mut headers = vec![RequestHeader::new(b"x-waf-version", version)];
    if case.response_deny {
        headers.push(RequestHeader::new(b"x-waf-response-deny", b"trigger"));
    }
    let head = RequestHead::new(
        b"POST /test/waf-dynamic HTTP/1.1",
        b"POST",
        b"/test/waf-dynamic",
        b"/test/waf-dynamic",
        b"HTTP/1.1",
        b"127.0.0.1",
        b"waf-dynamic.example.com",
        &headers,
    );
    let mode = match config.mode {
        edgion_resources::resources::edgion_plugins::plugin_configs::WafMode::On => RuleEngineMode::On,
        edgion_resources::resources::edgion_plugins::plugin_configs::WafMode::DetectionOnly => {
            RuleEngineMode::DetectionOnly
        }
    };
    let mut transaction = Transaction::new_with_modes(
        &generation,
        head,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        mode,
    )
    .map_err(|error| error.to_string())?;
    let mut observed_rule = None;
    let mut outcome = transaction
        .execute_request_headers()
        .map_err(|error| error.to_string())?;
    if let Some(status) = record(&outcome, &mut observed_rule) {
        return Ok((status, observed_rule));
    }
    transaction
        .publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(case.body.as_bytes()),
            BodyProcessorSelection::Auto,
        ))
        .map_err(|error| error.to_string())?;
    outcome = transaction.execute_request_body().map_err(|error| error.to_string())?;
    if let Some(status) = record(&outcome, &mut observed_rule) {
        return Ok((status, observed_rule));
    }
    transaction
        .publish_response_head(ResponseHead::new(200, &[]))
        .map_err(|error| error.to_string())?;
    outcome = transaction
        .execute_response_headers()
        .map_err(|error| error.to_string())?;
    if let Some(status) = record(&outcome, &mut observed_rule) {
        return Ok((status, observed_rule));
    }
    outcome = transaction
        .execute_response_window(ResponseWindow::new(&[], true))
        .map_err(|error| error.to_string())?;
    record(&outcome, &mut observed_rule);
    Ok((200, observed_rule))
}

fn record(outcome: &PhaseOutcome, observed_rule: &mut Option<u64>) -> Option<u16> {
    if observed_rule.is_none() {
        *observed_rule = outcome.findings().first().map(|finding| finding.rule_id());
    }
    outcome.intervention().map(|intervention| intervention.status())
}

fn run(manifest_path: &Path, report_path: &Path) -> Result<(), String> {
    let manifest_bytes = fs::read(manifest_path).map_err(|error| format!("manifest: {error}"))?;
    if sha256(&manifest_bytes) != REVIEWED_MANIFEST_SHA256 {
        return Err("reviewed acceptance manifest changed".to_string());
    }
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
    if manifest.schema != "edgion-waf-acceptance/v1" || manifest.profiles.len() != 3 || manifest.cases.len() < 9 {
        return Err("acceptance manifest schema or required coverage is invalid".to_string());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("workspace root is missing")?;
    let excluded = manifest
        .exclusions
        .iter()
        .map(|item| item.case_id.as_str())
        .collect::<BTreeSet<_>>();
    if excluded.len() != manifest.exclusions.len()
        || manifest
            .exclusions
            .iter()
            .any(|item| item.reason.is_empty() || item.revisit_when.is_empty())
    {
        return Err("acceptance exclusions must be unique and explained".to_string());
    }
    let mut compiled = Vec::new();
    let mut profile_names = BTreeSet::new();
    for profile in &manifest.profiles {
        if !profile_names.insert(profile.name.as_str()) {
            return Err(format!("duplicate profile: {}", profile.name));
        }
        let bytes = fs::read(root.join(&profile.path)).map_err(|error| format!("{}: {error}", profile.path))?;
        if sha256(&bytes) != profile.sha256 {
            return Err(format!("profile digest changed: {}", profile.path));
        }
        let mut document: PluginDocument = serde_yaml::from_slice(&bytes).map_err(|error| error.to_string())?;
        let policy_bytes =
            fs::read(root.join(&profile.policy.path)).map_err(|error| format!("{}: {error}", profile.policy.path))?;
        if sha256(&policy_bytes) != profile.policy.sha256 {
            return Err(format!("policy file digest changed: {}", profile.policy.path));
        }
        let policy_value: serde_yaml::Value =
            serde_yaml::from_slice(&policy_bytes).map_err(|error| error.to_string())?;
        if policy_value["spec"]["data"]["type"].as_str() != Some("WafPolicy")
            || policy_value["metadata"]["name"].as_str() != Some(document.spec.waf.policy_ref.name.as_str())
        {
            return Err(format!("policy reference changed: {}", profile.policy.path));
        }
        let policy: WafPolicyData = serde_yaml::from_value(policy_value["spec"]["data"]["config"].clone())
            .map_err(|error| error.to_string())?;
        policy.validate_shape()?;
        let selected = policy.default_profile.clone();
        let refs = &policy.profiles[&selected].bundle_refs;
        if refs.len() != profile.bundles.len() {
            return Err(format!("bundle inventory changed: {}", profile.name));
        }
        let mut resolved = Vec::new();
        for (reference, bundle_file) in refs.iter().zip(&profile.bundles) {
            let bytes =
                fs::read(root.join(&bundle_file.path)).map_err(|error| format!("{}: {error}", bundle_file.path))?;
            if sha256(&bytes) != bundle_file.sha256 {
                return Err(format!("bundle file digest changed: {}", bundle_file.path));
            }
            let value: serde_yaml::Value = serde_yaml::from_slice(&bytes).map_err(|error| error.to_string())?;
            let data: WafRuleBundleData =
                serde_yaml::from_value(value["spec"]["data"]["config"].clone()).map_err(|error| error.to_string())?;
            data.validate()?;
            let computed_digest = data.compute_digest();
            if reference.expected_digest.as_deref() != Some(computed_digest.as_str()) {
                return Err(format!("bundle reference digest changed: {}", bundle_file.path));
            }
            let namespace = reference.target.namespace.as_deref().unwrap_or("edgion-default");
            resolved.push(ResolvedWafBundle {
                identity: format!("{namespace}/{}", reference.target.name),
                digest: computed_digest,
                data,
            });
        }
        document.spec.waf.selected_profile = Some(selected);
        document.spec.waf.resolved_policy = Some(policy);
        document.spec.waf.owner_namespace = Some("edgion-default".to_string());
        document.spec.waf.resolved_ref_indices = (0..resolved.len()).collect();
        document.spec.waf.resolved_bundles = Some(resolved);
        let generation = document.spec.waf.compile_generation()?;
        let ids = generation.rule_units().map(|unit| unit.id()).collect::<Vec<_>>();
        if ids != profile.rule_ids || ids.is_empty() {
            return Err(format!("strict profile rule IDs changed: {}", profile.name));
        }
        compiled.push((profile.name.as_str(), document.spec.waf));
    }
    let mut results = Vec::new();
    let mut case_ids = BTreeSet::new();
    let mut false_positives = 0;
    let mut false_negatives = 0;
    for case in &manifest.cases {
        if !case_ids.insert(case.id.as_str()) || excluded.contains(case.id.as_str()) {
            return Err(format!("duplicate or excluded acceptance case: {}", case.id));
        }
        let config = compiled
            .iter()
            .find(|(name, _)| *name == case.profile)
            .ok_or("case refers to unknown profile")?;
        let result = observe(&config.1, case);
        let (actual_status, actual_rule_id, error) = match result {
            Ok((status, rule)) => (Some(status), rule, None),
            Err(error) => (None, None, Some(error)),
        };
        let passed = actual_status == Some(case.expected_status) && actual_rule_id == case.expected_rule_id;
        if !passed {
            false_positives +=
                usize::from(case.expected_status == 200 && actual_status.is_some_and(|status| status != 200));
            false_negatives += usize::from(case.expected_status != 200 && actual_status == Some(200));
        }
        results.push(CaseResult {
            id: &case.id,
            profile: &case.profile,
            expected_status: case.expected_status,
            actual_status,
            expected_rule_id: case.expected_rule_id,
            actual_rule_id,
            passed,
            error,
        });
    }
    for (id, profile, status, rule_id) in BASELINE_CASES {
        if !manifest.cases.iter().any(|case| {
            case.id == id
                && case.profile == profile
                && case.expected_status == status
                && case.expected_rule_id == rule_id
        }) {
            return Err(format!("baseline acceptance case changed or disappeared: {id}"));
        }
    }
    let passed = results.iter().filter(|item| item.passed).count();
    if !manifest
        .profiles
        .iter()
        .all(|profile| manifest.cases.iter().any(|case| case.profile == profile.name))
        || !manifest
            .cases
            .iter()
            .any(|case| case.expected_status == 200 && case.expected_rule_id.is_none())
        || !manifest.cases.iter().any(|case| case.expected_status != 200)
    {
        return Err("every profile and both allow/deny decisions need acceptance cases".to_string());
    }
    let report = Report {
        schema: "edgion-waf-acceptance-report/v1",
        profile: &manifest.profile,
        corpus_revision: &manifest.corpus_revision,
        manifest_sha256: sha256(&manifest_bytes),
        total: manifest.cases.len() + manifest.exclusions.len(),
        supported: manifest.cases.len(),
        executed: results.len(),
        passed,
        failed: results.len() - passed,
        excluded: manifest.exclusions.len(),
        false_positives,
        false_negatives,
        cases: results,
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(report_path, bytes).map_err(|error| format!("report: {error}"))?;
    if report.failed != 0 || report.excluded != 0 || report.supported == 0 {
        return Err(format!(
            "acceptance failed: {} cases failed, {} excluded",
            report.failed, report.excluded
        ));
    }
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let manifest = args.next().expect("usage: waf_production_acceptance MANIFEST REPORT");
    let report = args.next().expect("usage: waf_production_acceptance MANIFEST REPORT");
    assert!(args.next().is_none(), "unexpected argument");
    if let Err(error) = run(Path::new(&manifest), Path::new(&report)) {
        eprintln!("WAF production acceptance failed: {error}");
        std::process::exit(1);
    }
}
