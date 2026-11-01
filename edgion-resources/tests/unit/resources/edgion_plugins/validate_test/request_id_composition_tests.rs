use crate::resources::edgion_plugins::EdgionPlugins;

fn validate(yaml: &str) -> Vec<String> {
    let plugins: EdgionPlugins = yaml_serde::from_str(yaml).expect("deserialize EdgionPlugins");
    plugins.validate_plugins()
}

#[test]
fn early_attachment_rejects_late_sources_and_inert_entries_do_not_participate() {
    for fragment in [
        "config: {ids: [{header: x-id, source: {type: ctx, name: order}}]}",
        "config: {}\n      conditions: {run: {allOf: [{type: keyExist, key: {type: remoteIp}}]}}",
    ] {
        let yaml = format!("apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata: {{name: early}}\nspec:\n  requestPlugins:\n    - type: RequestId\n      {fragment}\n");
        let mut plugins: EdgionPlugins = yaml_serde::from_str(&yaml).unwrap();
        assert!(plugins.has_active_request_id());
        assert_eq!(plugins.validate_request_id_early_attachment().len(), 1);
        plugins.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        assert!(!plugins.has_active_request_id());
        assert!(plugins.validate_request_id_early_attachment().is_empty());
    }
}

#[test]
fn allows_two_request_id_entries() {
    let errs = validate(
        r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dup
  namespace: default
spec:
  requestPlugins:
    - type: RequestId
      config: {}
    - type: RequestId
      config: {}
"#,
    );
    assert!(errs.is_empty(), "incremental RequestId entries must be valid: {errs:?}");
}

#[test]
fn allows_single_request_id() {
    let errs = validate(
        r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ok
  namespace: default
spec:
  requestPlugins:
    - type: RequestId
      config: {}
"#,
    );
    assert!(errs.is_empty(), "unexpected duplicate error: {errs:?}");
}
