use super::*;

#[test]
fn default_has_no_timeout_template() {
    let c = ExternalHttpConnection::default();
    assert!(c.timeout_ms_template.is_none());
}

#[test]
fn valid_template_passes_validation() {
    let c: ExternalHttpConnection = yaml_serde::from_str(
        "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${ctx:desired_timeout_ms}\"\n",
    )
    .unwrap();
    assert_eq!(c.timeout_ms_template.as_deref(), Some("${ctx:desired_timeout_ms}"));
    assert!(validate_connection_target(&c).is_none());
}

#[test]
fn malformed_template_is_rejected_at_admission() {
    // Unclosed "${" must be caught by CompiledTemplate::compile at validation
    // time, not silently at runtime.
    let c: ExternalHttpConnection =
        yaml_serde::from_str("target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${ctx:X-Test\"\n").unwrap();
    let err = validate_connection_target(&c).expect("malformed template must be rejected");
    assert!(err.contains("timeoutMsTemplate"), "got: {err}");
}

#[test]
fn secret_ref_in_template_is_rejected_at_admission() {
    // ${secretRef:...} is never resolved for timeoutMsTemplate (not scanned by
    // collect_secret_ref_names nor the forward_auth credential resolver), so a
    // config that references one would silently fall back to the static
    // timeoutMs at runtime with no visible error. Reject it at admission instead.
    let c: ExternalHttpConnection = yaml_serde::from_str(
        "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"${secretRef:ns/name:key}\"\n",
    )
    .unwrap();
    let err = validate_connection_target(&c).expect("secretRef in timeoutMsTemplate must be rejected");
    assert!(err.contains("timeoutMsTemplate"), "got: {err}");
    assert!(err.contains("secretRef"), "got: {err}");
}

/// CRITICAL 1 regression: a client-controlled var in `timeoutMsTemplate` is an
/// auth-bypass / DoS vector on forward_auth (a client-chosen 1ms deadline makes
/// the auth call always time out). `${header:...}`, `${query:...}`,
/// `${cookie:...}`, and `${method}` must all be rejected at admission.
#[test]
fn timeout_template_rejects_client_controlled_vars() {
    for expr in [
        "${header:X-Auth-Timeout}",
        "${query:timeout}",
        "${cookie:timeout}",
        "${method}",
        "${path}",
        "${uri}",
        "${requestPath}",
        "${requestUri}",
    ] {
        let c: ExternalHttpConnection = yaml_serde::from_str(&format!(
            "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"{expr}\"\n"
        ))
        .unwrap();
        let err =
            validate_connection_target(&c).unwrap_or_else(|| panic!("{expr} must be rejected in timeoutMsTemplate"));
        assert!(err.contains("timeoutMsTemplate"), "got: {err}");
        assert!(err.contains("client-controlled"), "got: {err}");
    }
}

/// Trusted variable types must still be accepted for `timeoutMsTemplate`.
#[test]
fn timeout_template_accepts_trusted_vars() {
    for expr in [
        "${ctx:is_staging}",
        "${remoteIp}",
        "${matchedHttpRoute:name}",
        "${matchedGrpcRoute:name}",
    ] {
        let c: ExternalHttpConnection = yaml_serde::from_str(&format!(
            "target:\n  url: https://api.example.com\ntimeoutMsTemplate: \"{expr}\"\n"
        ))
        .unwrap();
        assert!(
            validate_connection_target(&c).is_none(),
            "{expr} should be accepted in timeoutMsTemplate"
        );
    }
}
