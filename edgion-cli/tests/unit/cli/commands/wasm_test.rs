use super::*;
use crate::cli::{Cli, Commands};
use clap::Parser;
use tempfile::TempDir;

const LOCAL: &str = r#"
name: ""
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
  code:
    local:
      filename: ./demo.wasm
"#;

fn converted(input: &str) -> ConversionOutcome {
    convert_envoy(input.as_bytes())
}

fn has_code(outcome: &ConversionOutcome, code: &str) -> bool {
    outcome.report.findings.iter().any(|finding| finding.code == code)
}

fn blocked(input: &str, code: &str) {
    let outcome = converted(input);
    assert!(!outcome.report.compatible, "expected {code}");
    assert!(!outcome.report.output_generated);
    assert!(outcome.config.is_none());
    assert!(
        has_code(&outcome, code),
        "missing {code}: {:?}",
        outcome.report.findings
    );
}

#[test]
fn clap_parses_nested_commands() {
    let cli = Cli::try_parse_from([
        "edgion-cli",
        "wasm",
        "inspect",
        "--file",
        "demo.wasm",
        "--format",
        "json",
    ])
    .expect("inspect parses");
    assert!(matches!(
        cli.command,
        Commands::Wasm(WasmArgs {
            command: WasmCommand::Inspect {
                format: InspectFormat::Json,
                ..
            }
        })
    ));
    Cli::try_parse_from([
        "edgion-cli",
        "wasm",
        "convert-envoy",
        "--file",
        "envoy.yaml",
        "--format",
        "yaml",
        "--report",
        "report.json",
    ])
    .expect("convert parses");
}

#[tokio::test]
async fn local_command_short_circuits_client_and_auth() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("demo.wasm");
    fs::write(
        &path,
        wat::parse_str(r#"(module (func (export "proxy_abi_version_0_2_0")))"#).expect("WAT"),
    )
    .expect("write");
    let cli = Cli::try_parse_from([
        "edgion-cli",
        "--server",
        "invalid server URL",
        "--token",
        "must-not-be-resolved",
        "wasm",
        "inspect",
        "--file",
        path.to_str().expect("path"),
    ])
    .expect("parse");
    cli.run().await.expect("must remain local");
}

#[test]
fn inspect_formats_compatible_incompatible_and_malformed_reports() {
    let compatible =
        edgion_wasm::analyze(&wat::parse_str(r#"(module (func (export "proxy_abi_version_0_2_0")))"#).expect("WAT"));
    assert!(compatible.compatible);
    assert!(serde_json::to_string(&compatible)
        .expect("JSON")
        .contains("\"reportVersion\":1"));
    assert!(serde_yaml::to_string(&compatible)
        .expect("YAML")
        .contains("reportVersion: 1"));
    assert!(inspect_text(&compatible).starts_with("reportVersion: 1\ncompatible: true\n"));

    let incompatible = edgion_wasm::analyze(
        &wat::parse_str(
            r#"(module (import "env" "unknown" (func))
                    (func (export "proxy_abi_version_0_2_0")))"#,
        )
        .expect("WAT"),
    );
    assert!(!incompatible.compatible);
    assert!(inspect_text(&incompatible).contains("host import is unknown"));
    assert!(!edgion_wasm::analyze(b"not wasm").compatible);
}

#[test]
fn accepts_wrapper_bare_snake_and_camel_and_rejects_wrong_filter() {
    let wrapper = format!(
        "name: envoy.filters.http.wasm\ntyped_config:\n  \"@type\": {ENVOY_HTTP_WASM_TYPE}\n  config:\n{}",
        indent(LOCAL, 4)
    );
    assert!(converted(&wrapper).report.compatible);
    let empty_name = converted(LOCAL);
    assert!(empty_name.report.compatible);
    assert!(has_code(&empty_name, "plugin_identity_default_differs"));
    let camel = r#"{"vmConfig":{"runtime":"envoy.wasm.runtime.wasmtime","code":{"local":{"inlineString":"abc"}}}}"#;
    assert!(converted(camel).report.compatible);
    blocked(
        &wrapper.replace(ENVOY_HTTP_WASM_FILTER, "envoy.filters.http.router"),
        "wrong_http_filter_name",
    );
    blocked(
        &LOCAL.replace("name: \"\"", "name: guest-visible-name"),
        "plugin_identity_unsupported",
    );
}

#[test]
fn maps_local_inline_and_exact_wrapper_bytes() {
    let inline = converted(
        r#"
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
  code: { local: { inline_bytes: "AGFi/w==" } }
  configuration:
    "@type": type.googleapis.com/google.protobuf.BytesValue
    value: "/wBh"
configuration:
  "@type": type.googleapis.com/google.protobuf.StringValue
  value: "plain UTF-8"
"#,
    );
    assert!(inline.report.compatible, "{:?}", inline.report.findings);
    let config = inline.config.expect("config");
    assert_eq!(
        config
            .plugin_config
            .as_ref()
            .expect("plugin")
            .guest_bytes()
            .expect("bytes"),
        b"plain UTF-8"
    );
    assert_eq!(
        config.vm_config.as_ref().expect("VM").guest_bytes().expect("bytes"),
        b"\xff\0a"
    );
    config.validate().expect("output validates");
}

#[test]
fn blocks_any_unknown_and_oneof_loss() {
    for secondary_type in [
        "type.googleapis.com/google.protobuf.BytesValue",
        "type.googleapis.com/google.protobuf.StringValue",
    ] {
        blocked(
                &LOCAL.replace(
                    "name: \"\"",
                    &format!(
                        "configuration:\n  \"@type\": type.googleapis.com/google.protobuf.StringValue\n  type_url: {secondary_type}\n  value: marker"
                    ),
                ),
                "any_type_conflict",
            );
    }
    blocked(
        &LOCAL.replace(
            "name: \"\"",
            "configuration:\n  \"@type\": type.googleapis.com/google.protobuf.Struct\n  value: \"{}\"",
        ),
        "struct_any_unsupported",
    );
    blocked(
        &LOCAL.replace(
            "name: \"\"",
            "configuration:\n  \"@type\": type.googleapis.com/example.Other\n  value: marker",
        ),
        "arbitrary_any_unsupported",
    );
    blocked(
        &LOCAL.replace("name: \"\"", "unknown_field: true"),
        "input_schema_invalid",
    );
    blocked(
        r#"
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
  code:
    local: { filename: demo.wasm }
    remote:
      http_uri: { uri: "https://example.test/demo.wasm", timeout: "1s" }
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        "code_source_oneof_conflict",
    );
    blocked(
        &LOCAL.replace("filename: ./demo.wasm", "filename: a\n      inline_string: b"),
        "local_source_oneof_conflict",
    );
}

#[test]
fn report_finish_never_marks_missing_output_compatible() {
    let outcome = ReportBuilder::new(b"future incomplete conversion").finish(None);
    assert!(!outcome.report.compatible);
    assert!(!outcome.report.output_generated);
    assert!(outcome.config.is_none());
    assert!(has_code(&outcome, "output_not_generated"));
}

#[test]
fn blocks_nonportable_security_and_lifecycle_fields() {
    let cases = [
        (
            LOCAL.replace("name: \"\"", "root_id: shared"),
            "context_identity_unsupported",
        ),
        (
            LOCAL.replace("runtime: envoy.wasm.runtime.wasmtime", "runtime: envoy.wasm.runtime.v8"),
            "runtime_unsupported",
        ),
        (
            LOCAL.replace("name: \"\"", "capability_restriction_config: {}"),
            "capability_restrictions_unsupported",
        ),
        (
            LOCAL.replace(
                "runtime: envoy.wasm.runtime.wasmtime",
                "runtime: envoy.wasm.runtime.wasmtime\n  environment_variables: {}",
            ),
            "environment_variables_unsupported",
        ),
        (
            LOCAL.replace(
                "runtime: envoy.wasm.runtime.wasmtime",
                "runtime: envoy.wasm.runtime.wasmtime\n  allow_precompiled: true",
            ),
            "allow_precompiled_unsupported",
        ),
        (
            LOCAL.replace(
                "runtime: envoy.wasm.runtime.wasmtime",
                "runtime: envoy.wasm.runtime.wasmtime\n  nack_on_code_cache_miss: true",
            ),
            "nack_on_cache_miss_unsupported",
        ),
        (
            LOCAL.replace("name: \"\"", "failure_policy: FAIL_RELOAD"),
            "fail_reload_unsupported",
        ),
        (
            LOCAL.replace("name: \"\"", "reload_config: {}"),
            "reload_config_unsupported",
        ),
        (
            LOCAL.replace("name: \"\"", "allow_on_headers_stop_iteration: false"),
            "headers_stop_iteration_unsupported",
        ),
        (
            LOCAL.replace("filename: ./demo.wasm", "environment_variable: WASM_CODE"),
            "local_environment_variable_unsupported",
        ),
        (
            LOCAL.replace(
                "filename: ./demo.wasm",
                "filename: ./demo.wasm\n      watched_directory: {}",
            ),
            "watched_directory_unsupported",
        ),
    ];
    for (input, code) in cases {
        blocked(&input, code);
    }
}

#[test]
fn maps_envoy_failure_policy_and_rejects_legacy_conflicts() {
    let cases = [
        (None, false, false, true),
        (None, true, true, true),
        (Some("UNSPECIFIED"), false, false, true),
        (Some("UNSPECIFIED"), true, true, true),
        (Some("FAIL_CLOSED"), false, false, true),
        (Some("FAIL_CLOSED"), true, false, false),
        (Some("FAIL_OPEN"), false, true, true),
        (Some("FAIL_OPEN"), true, false, false),
    ];

    for (policy, legacy_fail_open, expected_fail_open, expected_compatible) in cases {
        let mut input = LOCAL.replace("name: \"\"", &format!("name: \"\"\nfail_open: {legacy_fail_open}"));
        if let Some(policy) = policy {
            input = input.replace("name: \"\"", &format!("name: \"\"\nfailure_policy: {policy}"));
        }

        let outcome = converted(&input);
        assert_eq!(
            outcome.report.compatible, expected_compatible,
            "policy={policy:?}, legacy_fail_open={legacy_fail_open}: {:?}",
            outcome.report.findings
        );
        if expected_compatible {
            assert_eq!(
                outcome.config.expect("config").fail_open,
                expected_fail_open,
                "policy={policy:?}, legacy_fail_open={legacy_fail_open}"
            );
        } else {
            assert!(has_code(&outcome, "failure_policy_conflict"));
            assert!(outcome.config.is_none());
        }
    }

    blocked(
        &LOCAL.replace("name: \"\"", "name: \"\"\nfail_open: true\nfailure_policy: FAIL_RELOAD"),
        "failure_policy_conflict",
    );
    blocked(
        &LOCAL.replace(
            "name: \"\"",
            "name: \"\"\nfail_open: true\nfailure_policy: FUTURE_POLICY",
        ),
        "failure_policy_conflict",
    );
}

#[test]
fn blocks_remote_hazards_and_report_redacts_values() {
    let base = r#"
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
  code:
    remote:
      http_uri: { uri: "https://example.test/demo.wasm", cluster: artifact, timeout: "1s" }
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#;
    blocked(
        r#"
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
  code:
    remote:
      http_uri:
        uri: "https://example.test/demo.wasm"
        cluster: artifact
        timeout: "1s"
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        "remote_cluster_unsupported",
    );
    blocked(&base.replace(", cluster: artifact", ""), "remote_cluster_required");
    blocked(
        &base.replace("cluster: artifact", "cluster: \"\""),
        "remote_cluster_required",
    );
    blocked(&base.replace(", timeout: \"1s\"", ""), "remote_timeout_required");
    blocked(
        &base.replace("timeout: \"1s\"", "timeout: \"0s\""),
        "remote_timeout_out_of_range",
    );
    blocked(&base.replace("timeout: \"1s\"", "timeout: invalid"), "duration_invalid");
    blocked(
        &base.replace(
            "      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
            "",
        ),
        "remote_sha256_required",
    );
    blocked(
        &base.replace(
            "sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256: \"\"",
        ),
        "remote_sha256_required",
    );
    blocked(
        &base.replace("https://example.test", "file:///tmp"),
        "remote_uri_scheme_unsupported",
    );
    let outcome = converted(&base.replace("example.test", "user:report-redaction-marker@example.test"));
    assert!(has_code(&outcome, "remote_uri_userinfo_forbidden"));
    let report = serde_json::to_string(&outcome.report).expect("report");
    assert!(!report.contains("report-redaction-marker"));
}

#[test]
fn empty_runtime_warns_and_output_roundtrips() {
    let outcome = converted(
        r#"
name: ""
vm_config:
  code:
    local:
      filename: ./demo.wasm
"#,
    );
    assert!(outcome.report.compatible, "{:?}", outcome.report.findings);
    assert!(has_code(&outcome, "runtime_defaulted_to_wasmtime"));
    assert!(has_code(&outcome, "callouts_disabled_by_default"));
    let json = serde_json::to_string(&outcome.config.expect("config")).expect("JSON");
    let config: WasmConfig = serde_json::from_str(&json).expect("WasmConfig");
    config.validate().expect("valid");
}

#[test]
fn report_create_new_and_size_caps_are_enforced() {
    let temp = TempDir::new().expect("temp dir");
    let report = temp.path().join("report.json");
    write_create_new(&report, br#"{"version":1}"#).expect("write");
    let original = fs::read(&report).expect("read");
    assert!(write_create_new(&report, br#"{"version":2}"#).is_err());
    assert_eq!(fs::read(&report).expect("read"), original);

    let module = temp.path().join("large.wasm");
    File::create(&module)
        .expect("create")
        .set_len(MAX_WASM_MODULE_BYTES + 1)
        .expect("sparse");
    assert!(read_bounded(&module, MAX_WASM_MODULE_BYTES, "Wasm module").is_err());
    let input = temp.path().join("large.yaml");
    File::create(&input)
        .expect("create")
        .set_len(MAX_ENVOY_INPUT_BYTES + 1)
        .expect("sparse");
    assert!(read_bounded(&input, MAX_ENVOY_INPUT_BYTES, "Envoy conversion input").is_err());
}

#[test]
fn missing_code_blocks_pure_conversion_and_cli_report_path() {
    let input = r#"
vm_config:
  runtime: envoy.wasm.runtime.wasmtime
"#;
    let outcome = converted(input);
    assert!(!outcome.report.compatible);
    assert!(!outcome.report.output_generated);
    assert!(outcome.config.is_none());
    assert!(has_code(&outcome, "missing_code"));

    let temp = TempDir::new().expect("temp dir");
    let input_path = temp.path().join("envoy.yaml");
    let report_path = temp.path().join("report.json");
    fs::write(&input_path, input).expect("write input");
    assert!(convert_envoy_file(&input_path, ConfigFormat::Json, Some(&report_path)).is_err());
    let report: EnvoyWasmConversionReport =
        serde_json::from_slice(&fs::read(report_path).expect("read report")).expect("parse report");
    assert!(!report.compatible);
    assert!(!report.output_generated);
    assert!(report.findings.iter().any(|finding| finding.code == "missing_code"));
}

fn indent(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value.trim().lines().map(|line| format!("{prefix}{line}\n")).collect()
}
