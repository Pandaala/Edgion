use crate::resources::edgion_plugins::EdgionPlugins;

fn validate(yaml: &str) -> Vec<String> {
    let plugins: EdgionPlugins = yaml_serde::from_str(yaml).expect("deserialize EdgionPlugins");
    plugins.validate_plugins()
}

/// A disabled entry whose dormant `conditions` carry a malformed regex must
/// not produce any validation error — `enable: false` is semantically inert.
#[test]
fn disabled_entry_with_malformed_conditions_is_inert() {
    let errs = validate(
        r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: parked
  namespace: default
spec:
  requestPlugins:
    - type: RequestHeaderModifier
      enable: false
      config: {}
      conditions:
        run:
          allOf:
            - type: keyMatch
              key:
                type: header
                name: X-Test
              regex: ["("]
"#,
    );
    assert!(
        errs.is_empty(),
        "disabled entry must not surface a condition error, got: {errs:?}"
    );
}

/// Enabling the same malformed entry deterministically surfaces the
/// condition regex error — the disabled/enabled boundary is the only switch.
#[test]
fn enabling_the_same_entry_surfaces_the_condition_error() {
    let errs = validate(
        r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: active
  namespace: default
spec:
  requestPlugins:
    - type: RequestHeaderModifier
      enable: true
      config: {}
      conditions:
        run:
          allOf:
            - type: keyMatch
              key:
                type: header
                name: X-Test
              regex: ["("]
"#,
    );
    assert!(
        errs.iter().any(|e| e.contains("conditions regex")),
        "enabled entry must surface the condition regex error, got: {errs:?}"
    );
}
