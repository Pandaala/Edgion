use super::*;
use crate::output::{LocalFileWriterCfg, StringOutput};

#[test]
fn log_config_without_fallback_is_backward_compatible() {
    // Old configs have no `fallback` key — must deserialize to None.
    let json = r#"{"enabled":true,"output":{"localFile":{"path":"logs/a.log"}}}"#;
    let cfg: LogConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.fallback.is_none());
    // `skip_serializing_if = "Option::is_none"` must keep the key out of the
    // serialized form, so existing YAML/JSON configs never grow a stale
    // `"fallback": null`.
    let reserialized = serde_json::to_string(&cfg).unwrap();
    assert!(
        !reserialized.contains("fallback"),
        "None fallback must be omitted: {reserialized}"
    );
}

#[test]
fn log_config_with_fallback_roundtrips() {
    let cfg = LogConfig {
        enabled: true,
        output: StringOutput::Es {
            link_sys_ref: crate::output::LinkSysRef {
                namespace: "default".into(),
                name: "es-main".into(),
            },
        },
        fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
            path: "logs/es_fallback.log".into(),
            ..Default::default()
        })),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: LogConfig = serde_json::from_str(&json).unwrap();
    // Assert the inner config survived the roundtrip, not just the variant —
    // a variant-only check would pass even if the payload were reset to
    // defaults. StringOutput has no PartialEq, so match and compare `path`.
    match back.fallback {
        Some(StringOutput::LocalFile(ref f)) => assert_eq!(f.path, "logs/es_fallback.log"),
        other => panic!("expected LocalFile fallback, got {other:?}"),
    }
}

#[test]
fn access_log_options_accepts_only_the_canonical_camel_case_key() {
    let options: AccessLogOptions = serde_json::from_str(r#"{"includeRawUri":true}"#).unwrap();
    assert!(options.include_raw_uri);
    assert!(serde_json::from_str::<AccessLogOptions>(r#"{"include_raw_uri":true}"#).is_err());
}
