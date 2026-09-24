use super::parse_resource_content;
use edgion_resources::prelude_resources::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins, HTTPRoute, LinkSys};

const TYPO_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: typo-plugins
  namespace: default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        denys: []
"#;

const CLEAN_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: clean-plugins
  namespace: default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        deny: []
"#;

const TYPO_STREAM_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: typo-stream-plugins
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        denys: []
"#;

const TYPO_CONFIG_DATA: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: typo-config-data
  namespace: default
spec:
  data:
    type: IpList
    config:
      items:
        - name: office
          cidrs: []
          cidrz: [10.0.0.0/8]
"#;

const TYPO_PROXY_PROTOCOL_CONFIG_DATA: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: proxy-policy
  namespace: default
spec:
  data:
    type: ProxyProtocolTrust
    config:
      mode: required
      typo: true
"#;

const TYPO_LINK_SYS: &str = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: typo-redis
  namespace: default
spec:
  type: redis
  config:
    endpoints: [redis://127.0.0.1:6379]
    timeout:
      connect: 3s
      conect: 20
"#;

#[test]
fn edgion_plugins_typo_populates_unknown_fields_with_exact_path() {
    let parsed: EdgionPlugins = parse_resource_content(TYPO_PLUGINS).expect("well-formed aside from the typo");
    assert_eq!(
        parsed.spec.unknown_fields,
        vec!["requestPlugins[0].config.denys".to_string()]
    );
}

#[test]
fn edgion_plugins_clean_document_yields_no_findings() {
    let parsed: EdgionPlugins = parse_resource_content(CLEAN_PLUGINS).expect("clean document must parse");
    assert!(parsed.spec.unknown_fields.is_empty());
}

#[test]
fn edgion_stream_plugins_typo_populates_unknown_fields_with_exact_path() {
    let parsed: EdgionStreamPlugins =
        parse_resource_content(TYPO_STREAM_PLUGINS).expect("well-formed aside from the typo");
    assert_eq!(parsed.spec.unknown_fields, vec!["plugins[0].config.denys".to_string()]);
}

#[test]
fn edgion_stream_plugins_clean_document_yields_no_findings() {
    let clean = TYPO_STREAM_PLUGINS.replace("denys", "deny");
    let parsed: EdgionStreamPlugins = parse_resource_content(&clean).expect("clean document must parse");
    assert!(parsed.spec.unknown_fields.is_empty());
}

#[test]
fn edgion_config_data_typo_populates_unknown_fields_with_exact_path() {
    let parsed: EdgionConfigData = parse_resource_content(TYPO_CONFIG_DATA).expect("well-formed aside from the typo");
    assert_eq!(
        parsed.spec.unknown_fields,
        vec!["data.config.items[0].cidrz".to_string()]
    );
}

#[test]
fn proxy_protocol_config_data_typo_parses_and_is_fatal() {
    let parsed: EdgionConfigData = parse_resource_content(TYPO_PROXY_PROTOCOL_CONFIG_DATA)
        .expect("ProxyProtocolTrust typo must remain loadable for status reporting");
    assert_eq!(parsed.spec.unknown_fields, ["data.config.typo"]);
    assert_eq!(
        parsed.validate_pre_parse(),
        ["ProxyProtocolTrust invalid: policy validation failed"]
    );
}

#[test]
fn edgion_config_data_misc_payload_remains_open() {
    let misc = TYPO_CONFIG_DATA.replace(
            "type: IpList\n    config:\n      items:\n        - name: office\n          cidrs: []\n          cidrz: [10.0.0.0/8]",
            "type: Misc\n    config:\n      vendor:\n        arbitrary: [1, 2, 3]",
        );
    let parsed: EdgionConfigData = parse_resource_content(&misc).expect("Misc must accept arbitrary JSON");
    assert!(parsed.spec.unknown_fields.is_empty());
}

#[test]
fn link_sys_typo_populates_unknown_fields_with_exact_path() {
    let parsed: LinkSys = parse_resource_content(TYPO_LINK_SYS).expect("well-formed aside from the typo");
    assert_eq!(parsed.spec.unknown_fields, ["config.timeout.conect"]);
}

#[test]
fn link_sys_clean_and_removed_webhook_fields_report_findings() {
    let clean = TYPO_LINK_SYS.replace("      conect: 20\n", "");
    let parsed: LinkSys = parse_resource_content(&clean).expect("clean LinkSys must parse");
    assert!(parsed.spec.unknown_fields.is_empty());

    let webhook = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata: {name: legacy-webhook, namespace: default}
spec:
  type: webhook
  config:
    target: {kind: Service, name: resolver, port: 8080}
    auth: {vendor: {arbitrary: true}}
    defaultHeaders: {X-Legacy: {arbitrary: true}}
    requestMethod: {arbitrary: true}
"#;
    let parsed: LinkSys = parse_resource_content(webhook).expect("removed fields must not block parsing");
    assert_eq!(
        parsed.spec.unknown_fields,
        ["config.auth", "config.defaultHeaders", "config.requestMethod"]
    );
}

/// The non-fatal guarantee: a document that re-parses as a `serde_yaml::Value`
/// but cannot convert to `serde_json::Value` (a non-string mapping key, which
/// JSON has no representation for) must still load successfully via the normal
/// typed parse — the diagnostic just skips itself instead of failing the load.
/// This guarantee is tested directly against the seam helper
/// rather than through `parse_resource_content`'s full round trip: a
/// document with a mapping key that is itself a YAML sequence (`? [a, b]`)
/// parses fine into `serde_yaml::Value` (YAML permits non-scalar keys),
/// but JSON has no representation for a non-string object key, so
/// `serde_json::to_value` fails on it — the exact failure shape this
/// non-fatal seam guards against.
///
/// This cannot be exercised through `parse_resource_content`'s normal
/// entry point: a struct field's key deserializer requires a string
/// identifier regardless of whether the field is recognized, so any
/// document with a non-scalar top-level key fails the typed parse itself
/// (`serde_yaml::from_str::<K>`) before this seam ever runs — i.e. the
/// class of document that breaks the JSON round-trip is already rejected
/// upstream, and the seam's non-fatal skip is defense-in-depth for that
/// gap rather than something reachable end-to-end today. Calling the
/// helper directly with an already-parsed clean object isolates exactly
/// the required behavior: the check skips itself and leaves the
/// object's `unknown_fields` untouched.
#[test]
fn json_conversion_failure_is_skipped_and_leaves_object_untouched() {
    let mut parsed: EdgionPlugins = parse_resource_content(CLEAN_PLUGINS).expect("clean document must parse");
    let unconvertible_content = "? [a, b]\n: nested-key-seq\n";

    super::populate_unknown_fields_non_fatal(
        edgion_resources::ResourceKind::EdgionPlugins,
        unconvertible_content,
        &mut parsed,
    );

    assert!(
        parsed.spec.unknown_fields.is_empty(),
        "a YAML->JSON conversion failure must be skipped, not surfaced as spurious findings"
    );
}

/// An unsupported kind goes through `parse_resource_content` completely
/// unchanged — no findings machinery is invoked for it.
#[test]
fn unsupported_kind_is_unaffected() {
    let content = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  hostnames: [example.com]
"#;
    let parsed: HTTPRoute = parse_resource_content(content).expect("HTTPRoute must still parse normally");
    assert_eq!(parsed.metadata.name.as_deref(), Some("r1"));
}
