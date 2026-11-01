use super::*;

#[test]
fn mirror_global_defaults_and_partial_override() {
    let default: RequestMirrorGlobalConfig = yaml_serde::from_str("{}").unwrap();
    assert_eq!(
        default.resolve().unwrap(),
        ResolvedRequestMirrorGlobalConfig {
            max_concurrent: 1024,
            max_buffered_bytes: 64 * 1024 * 1024,
        }
    );
    let partial: RequestMirrorGlobalConfig = yaml_serde::from_str("max_buffered_bytes: 2MiB").unwrap();
    assert_eq!(partial.resolve().unwrap().max_concurrent, 1024);
    assert_eq!(partial.resolve().unwrap().max_buffered_bytes, 2 * 1024 * 1024);
}

#[test]
fn mirror_global_limits_accept_boundaries_and_reject_unbounded_values() {
    let largest_buffer = MirrorTuning::MAX_BUFFERED_BYTES.min(Semaphore::MAX_PERMITS).to_string();
    for (concurrent, bytes) in [(1, "1"), (MirrorTuning::MAX_CONCURRENT, largest_buffer.as_str())] {
        let config = RequestMirrorGlobalConfig {
            max_concurrent: concurrent,
            max_buffered_bytes: bytes.to_string(),
        };
        let resolved = config.resolve().unwrap();
        assert!(resolved.max_concurrent <= Semaphore::MAX_PERMITS);
        assert!(resolved.max_buffered_bytes <= Semaphore::MAX_PERMITS);
    }
    for concurrent in [0, MirrorTuning::MAX_CONCURRENT + 1, usize::MAX] {
        let config = RequestMirrorGlobalConfig {
            max_concurrent: concurrent,
            ..Default::default()
        };
        assert!(config.resolve().unwrap_err().contains("max_concurrent"));
    }
    for bytes in [
        "0",
        "-1",
        "1GiB1",
        "1073741825",
        "2GiB",
        "NaN",
        "",
        "999999999999999999999GiB",
    ] {
        let config = RequestMirrorGlobalConfig {
            max_buffered_bytes: bytes.to_string(),
            ..Default::default()
        };
        assert!(config.resolve().unwrap_err().contains("max_buffered_bytes"), "{bytes}");
    }
}

#[test]
fn mirror_global_unknown_knob_is_rejected() {
    assert!(yaml_serde::from_str::<RequestMirrorGlobalConfig>("max_concurent: 12").is_err());
}
