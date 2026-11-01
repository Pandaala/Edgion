use super::*;
use crate::resources::common::{CustomField, CustomSource, MethodSpec, OriginField, PathSpec, PresencePolicy};

#[test]
fn test_webhook_outbound_request_all_origin() {
    let yaml_str = r#"
args:
  origin:
    - name: tenant_id
    - name: trace_id
      presence: required
headers:
  origin:
    - name: X-Tenant-Id
cookies:
  origin:
    - name: session_id
"#;
    let request: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert_eq!(request.args.origin.len(), 2);
    assert_eq!(request.headers.origin.len(), 1);
    assert_eq!(request.cookies.origin.len(), 1);
    assert!(request.path.is_none());
}

#[test]
fn test_webhook_outbound_request_with_path_and_method() {
    let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args: {}
headers: {}
"#;
    let request: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert!(request.path.is_some());
    assert_eq!(request.path.as_ref().unwrap().template, "/webhook/${ctx:action}");
    assert!(request.path.as_ref().unwrap().allow_override);
    assert!(request.method.is_some());
    assert_eq!(request.method.as_ref().unwrap().template, "POST");
}

#[test]
fn test_custom_field_with_literal_value() {
    let yaml_str = r#"
- name: X-Source
  value: edgion
"#;
    let custom_fields: Vec<CustomField> = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert_eq!(custom_fields.len(), 1);
    assert_eq!(custom_fields[0].name, "X-Source");
    match &custom_fields[0].source {
        CustomSource::Value(val) => {
            assert_eq!(val, "edgion");
        }
        _ => panic!("Expected Value source"),
    }
}

#[test]
fn test_custom_field_with_template() {
    let yaml_str = r#"
- name: X-Action
  template: "${ctx:action}"
"#;
    let custom_fields: Vec<CustomField> = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert_eq!(custom_fields.len(), 1);
    assert_eq!(custom_fields[0].name, "X-Action");
    match &custom_fields[0].source {
        CustomSource::Template(tpl) => {
            assert_eq!(tpl, "${ctx:action}");
        }
        _ => panic!("Expected Template source"),
    }
}

#[test]
fn test_presence_policy_default() {
    let origin_field = OriginField {
        name: "X-Foo".to_string(),
        presence: Default::default(),
    };
    assert!(matches!(origin_field.presence, PresencePolicy::SkipIfMissing));
}

#[test]
fn test_presence_policy_required() {
    let yaml_str = r#"
- name: X-Required
  presence: required
"#;
    let fields: Vec<OriginField> = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert_eq!(fields.len(), 1);
    assert!(matches!(fields[0].presence, PresencePolicy::Required));
}

#[test]
fn test_presence_policy_warn() {
    let yaml_str = r#"
- name: X-Warn
  presence: warnIfMissing
"#;
    let fields: Vec<OriginField> = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert_eq!(fields.len(), 1);
    assert!(matches!(fields[0].presence, PresencePolicy::WarnIfMissing));
}

#[test]
fn test_webhook_outbound_request_all_custom() {
    let yaml_str = r#"
path:
  template: "/webhook/${ctx:action}"
  allowOverride: true
method:
  template: POST
args:
  custom:
    - name: source
      value: edgion
    - name: action
      template: "${ctx:action}"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-default/webhook-token:token}"
"#;
    let request: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    assert!(request.path.is_some());
    assert!(request.method.is_some());
    assert!(request.path.as_ref().unwrap().allow_override);
    assert_eq!(request.args.custom.len(), 2);
    assert_eq!(request.headers.custom.len(), 1);
}

#[test]
fn test_list_fields_default() {
    let fields: ListFields = Default::default();
    assert_eq!(fields.origin.len(), 0);
    assert_eq!(fields.custom.len(), 0);
}

#[test]
fn test_deny_unknown_fields_on_path_spec() {
    let yaml_str = r#"
template: "/webhook"
allowOverride: true
unknownField: value
"#;
    let result: Result<PathSpec, _> = yaml_serde::from_str(yaml_str);
    assert!(result.is_err());
}

#[test]
fn test_deny_unknown_fields_on_method_spec() {
    let yaml_str = r#"
template: "POST"
unknownField: value
"#;
    let result: Result<MethodSpec, _> = yaml_serde::from_str(yaml_str);
    assert!(result.is_err());
}

#[test]
fn test_deny_unknown_fields_on_webhook_outbound_request() {
    let yaml_str = r#"
path:
  template: "/webhook"
unknownField: value
"#;
    let result: Result<OutboundRequestSpec, _> = yaml_serde::from_str(yaml_str);
    assert!(result.is_err());
}

/// Build an otherwise-valid config so request-field validation is the only
/// thing that can fail.
fn valid_base_config() -> WebhookServiceConfig {
    let mut config = WebhookServiceConfig::default();
    config.conn.target.service.name = Some("webhook-svc".to_string());
    config.conn.target.service.port = Some(8080);
    config
}

fn origin(name: &str) -> OriginField {
    OriginField {
        name: name.to_string(),
        presence: PresencePolicy::SkipIfMissing,
    }
}

fn custom(name: &str) -> CustomField {
    CustomField {
        name: name.to_string(),
        source: CustomSource::Value("x".to_string()),
    }
}

#[test]
fn test_same_field_origin_and_custom_rejected() {
    // args: same name in origin and custom.
    let mut config = valid_base_config();
    config.request.args.origin = vec![origin("tenant_id")];
    config.request.args.custom = vec![custom("tenant_id")];
    let err = config.get_validation_error().expect("should reject");
    assert!(err.contains("appears in both origin and custom"), "got: {err}");

    // headers: case-insensitive collision (X-Trace-Id vs x-trace-id).
    let mut config = valid_base_config();
    config.request.headers.origin = vec![origin("X-Trace-Id")];
    config.request.headers.custom = vec![custom("x-trace-id")];
    let err = config.get_validation_error().expect("should reject");
    assert!(err.contains("appears in both origin and custom"), "got: {err}");

    // cookies: case-sensitive — distinct case must NOT collide.
    let mut config = valid_base_config();
    config.request.cookies.origin = vec![origin("Session")];
    config.request.cookies.custom = vec![custom("session")];
    assert!(
        config.get_validation_error().is_none(),
        "cookie names are case-sensitive; different case should be allowed"
    );
}

#[test]
fn test_duplicate_names_in_origin_rejected() {
    // args duplicate in origin.
    let mut config = valid_base_config();
    config.request.args.origin = vec![origin("trace_id"), origin("trace_id")];
    let err = config.get_validation_error().expect("should reject");
    assert!(
        err.contains("duplicate field name") && err.contains("origin"),
        "got: {err}"
    );

    // headers duplicate in origin, case-insensitive.
    let mut config = valid_base_config();
    config.request.headers.origin = vec![origin("X-Id"), origin("x-id")];
    let err = config.get_validation_error().expect("should reject");
    assert!(
        err.contains("duplicate field name") && err.contains("origin"),
        "got: {err}"
    );
}

#[test]
fn test_duplicate_names_in_custom_rejected() {
    let mut config = valid_base_config();
    config.request.headers.custom = vec![custom("Authorization"), custom("Authorization")];
    let err = config.get_validation_error().expect("should reject");
    assert!(
        err.contains("duplicate field name") && err.contains("custom"),
        "got: {err}"
    );
}

#[test]
fn test_cookie_both_origin_and_custom_rejected() {
    // headers.origin[name: Cookie] with cookies.origin present.
    let mut config = valid_base_config();
    config.request.headers.origin = vec![origin("Cookie")];
    config.request.cookies.origin = vec![origin("session_id")];
    let err = config.get_validation_error().expect("should reject");
    assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

    // case-insensitive header name "cookie" with cookies.custom present.
    let mut config = valid_base_config();
    config.request.headers.origin = vec![origin("cookie")];
    config.request.cookies.custom = vec![custom("token")];
    let err = config.get_validation_error().expect("should reject");
    assert!(err.contains("headers.origin[name:Cookie]"), "got: {err}");

    // Cookie header with no cookies.* configured is fine.
    let mut config = valid_base_config();
    config.request.headers.origin = vec![origin("Cookie")];
    assert!(config.get_validation_error().is_none(), "Cookie header alone is valid");
}

#[test]
fn collect_secret_ref_names_across_fields() {
    let yaml_str = r#"
path:
  template: "/wh/${secretRef:ns/p:k}"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-system/tok:token}"
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    let mut names = cfg.collect_secret_ref_names();
    names.sort();
    assert_eq!(names, vec!["edgion-system/tok:token".to_string(), "ns/p:k".to_string()]);
}

#[test]
fn unresolved_secret_refs_reports_missing() {
    let yaml_str = r#"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:edgion-system/tok:token}"
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    // No resolved_secrets -> reported unresolved.
    assert_eq!(
        cfg.unresolved_secret_refs(),
        vec!["edgion-system/tok:token".to_string()]
    );
}

#[test]
fn validation_rejects_malformed_secret_ref() {
    let yaml_str = r#"
headers:
  custom:
    - name: Authorization
      template: "Bearer ${secretRef:tok:token}"
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    let err = cfg.get_validation_error().expect("must reject no-namespace secretRef");
    assert!(err.contains("secretRef"), "got: {err}");
}

#[test]
fn resolved_secrets_debug_is_redacted() {
    let mut m = std::collections::BTreeMap::new();
    m.insert("ns/n:k".to_string(), "supersecret".to_string());
    let rs = ResolvedSecrets(m);
    let dbg = format!("{:?}", rs);
    assert!(!dbg.contains("supersecret"), "Debug must not print values: {dbg}");
    assert!(dbg.contains("redacted"), "Debug should mark redaction: {dbg}");
}

#[test]
fn resolved_secrets_redacts_under_guard_real_without() {
    use crate::redact::AdminRedactGuard;
    let mut m = std::collections::BTreeMap::new();
    m.insert("ns/n:k".to_string(), "supersecret".to_string());
    let wrapped: Option<ResolvedSecrets> = Some(ResolvedSecrets(m));

    // Under guard -> "[redacted]".
    {
        let _g = AdminRedactGuard::new();
        let json = serde_json::to_string(&RedactProbe {
            resolved_secrets: wrapped.clone(),
        })
        .unwrap();
        assert!(json.contains("[redacted]"), "guarded: {json}");
        assert!(!json.contains("supersecret"), "guarded leaked: {json}");
    }
    // Without guard -> real value (conf_sync transport).
    let json = serde_json::to_string(&RedactProbe {
        resolved_secrets: wrapped,
    })
    .unwrap();
    assert!(json.contains("supersecret"), "transport must ship real value: {json}");
}

#[derive(serde::Serialize)]
struct RedactProbe {
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    resolved_secrets: Option<ResolvedSecrets>,
}

#[test]
fn collect_secret_ref_names_includes_body_templates() {
    let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:prod/webhook:token}"}'
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    assert_eq!(cfg.collect_secret_ref_names(), vec!["prod/webhook:token".to_string()]);
}

#[test]
fn collect_secret_ref_names_includes_form_field_templates() {
    let yaml_str = r#"
body:
  type: form
  fields:
    - name: token
      template: "${secretRef:prod/webhook:token}"
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    assert_eq!(cfg.collect_secret_ref_names(), vec!["prod/webhook:token".to_string()]);
}

#[test]
fn validation_rejects_malformed_secret_ref_in_body() {
    // No namespace in the secretRef — validate_secret_ref_segments must reject.
    let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:tok:token}"}'
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    let err = cfg
        .get_validation_error()
        .expect("must reject malformed body secretRef");
    assert!(err.contains("secretRef"), "got: {err}");
}

#[test]
fn unresolved_secret_refs_reports_body_only_ref() {
    let yaml_str = r#"
body:
  type: json
  template: '{"token": "${secretRef:prod/webhook:token}"}'
"#;
    let req: OutboundRequestSpec = yaml_serde::from_str(yaml_str).expect("deserialize");
    let cfg = WebhookServiceConfig {
        request: req,
        ..valid_base_config()
    };
    assert_eq!(cfg.unresolved_secret_refs(), vec!["prod/webhook:token".to_string()]);
}
