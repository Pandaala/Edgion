use super::*;
use edgion_resources::{LinkSysRef, LocalFileWriterCfg, LogConfig};

#[test]
fn backfill_checks_es_identity_without_changing_disabled_or_fallback_capture() {
    let mut config = LogConfig {
        enabled: false,
        output: StringOutput::Es {
            link_sys_ref: LinkSysRef {
                namespace: "bad/ns".into(),
                name: "sink".into(),
            },
        },
        fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
            path: "r6f-fallback.log".into(),
            ..Default::default()
        })),
    };
    let invalid = derive_backfill_config(&config);
    assert_eq!(invalid.es_key, None);
    assert!(invalid
        .fallback_base_path
        .as_ref()
        .unwrap()
        .ends_with("r6f-fallback.log"));
    config.output = StringOutput::Es {
        link_sys_ref: LinkSysRef {
            namespace: "logs".into(),
            name: "sink".into(),
        },
    };
    let valid = derive_backfill_config(&config);
    assert_eq!(valid.es_key.as_deref(), Some("logs/sink"));
    assert_eq!(valid.fallback_base_path, invalid.fallback_base_path);
    config.output = StringOutput::LocalFile(LocalFileWriterCfg {
        path: String::new(),
        ..Default::default()
    });
    assert_eq!(derive_backfill_config(&config).es_key, None);
}
