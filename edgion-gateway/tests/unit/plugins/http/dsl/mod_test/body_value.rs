use super::*;

use super::super::lang::body_ops::DslBodyOps;
use super::super::lang::body_scan_budget::BodyScanBudget;

async fn facade_body_session(body: &[u8], memory_limit: usize) -> MockPluginSession {
    use bytes::Bytes;
    use pingora_core::protocols::http::RequestBodyBuffer;

    let cap = body.len() as u64;
    let (mut storage, view) = crate::request_body::BodyStorage::new(memory_limit, cap);
    storage.write(&Bytes::copy_from_slice(body)).await.unwrap();
    storage.finish().await.unwrap();
    let mut ctx = crate::EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = cap;
    ctx.request_body.install(view);

    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .withf(|known_empty| !known_empty)
        .return_const(Ok(cap));
    session.expect_ctx().return_const(ctx);
    session
}

#[tokio::test]
async fn body_facade_refunds_unfetched_chunks_before_returning_text_overflow() {
    let body = vec![0xff; 128 * 1024 + 1];
    let cap = body.len() as u64;
    let limits = VmLimits {
        max_string_len: body.len(),
        ..Default::default()
    };
    let runtime = DslRuntime {
        max_body_scan_passes: 1,
        ..Default::default()
    };

    for memory_limit in [body.len(), 1] {
        let mut session = facade_body_session(&body, memory_limit).await;
        let mut budget = BodyScanBudget::default();
        let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
        let error = ops.read_text_range(0, cap).await;
        assert!(matches!(error, Err(RuntimeError::StringTooLong { len, limit })
            if len == body.len() + 1 && limit == body.len()));
        assert_eq!(budget.reserve(cap, 1).unwrap(), cap - 64 * 1024);
    }
}

#[tokio::test]
async fn body_facade_read_length_preflight_precedes_scan_debit() {
    let body = vec![b'x'; 64];
    let limits = VmLimits {
        max_string_len: 32,
        ..Default::default()
    };
    let runtime = DslRuntime {
        max_body_scan_passes: 1,
        ..Default::default()
    };

    for memory_limit in [body.len(), 1] {
        let mut session = facade_body_session(&body, memory_limit).await;
        let mut budget = BodyScanBudget::default();
        budget.reserve_exact(1, 64, 1).unwrap();
        let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
        assert!(matches!(
            ops.read_text_range(0, 64).await,
            Err(RuntimeError::StringTooLong { len: 64, limit: 32 })
        ));
        let remaining = budget.reserve(64, 1).unwrap();
        assert_eq!(remaining, 63);
        budget.settle(remaining, 0);
        budget.reserve_exact(32, 64, 1).unwrap();

        let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
        assert!(matches!(
            ops.read_text_range(0, 32).await,
            Err(RuntimeError::BodyAccess { .. })
        ));
        assert_eq!(budget.reserve(64, 1).unwrap(), 0);
    }
}

#[tokio::test]
async fn body_read_requested_length_is_checked_without_storage_or_scan_access() {
    let limits = VmLimits::default();
    assert_eq!(limits.max_string_len, 8192);
    let runtime = DslRuntime::default();
    let mut session = MockPluginSession::new();
    session.expect_ctx().times(0);
    let mut budget = BodyScanBudget::default();
    budget.reserve_exact(1, 10, 1).unwrap();
    for requested in [8193, u64::MAX] {
        let result = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime)
            .read_text_range(0, requested)
            .await;
        assert!(matches!(result, Err(RuntimeError::StringTooLong { len, limit: 8192 })
            if len == usize::try_from(requested).unwrap_or(usize::MAX)));
    }
    assert_eq!(budget.reserve(10, 1).unwrap(), 9);
}

#[test]
fn body_read_accepts_exact_default_and_configured_length_but_never_clamps_oversize_requests() {
    for limit in [8192, 16384, 1048576] {
        let body = vec![b'x'; limit];
        for memory_limit in [body.len(), 1] {
            let limits = VmLimits {
                max_string_len: limit,
                ..Default::default()
            };
            assert_eq!(
                run_body_script_with_limits(
                    &format!("if req.body.read(0, {limit}).len() != {limit} {{ return deny(400, \"length\") }}"),
                    &body,
                    memory_limit,
                    1,
                    limits.clone(),
                )
                .unwrap(),
                PluginRunningResult::GoodNext
            );
            // A short body does not silently make an oversized argument valid.
            let requested = limit + 1;
            assert!(matches!(
                run_body_script_with_limits(
                    &format!("req.body.read(0, {requested})"),
                    b"x",
                    memory_limit,
                    1,
                    limits,
                ),
                Err(RuntimeError::StringTooLong { len, limit: actual_limit })
                    if len == requested && actual_limit == limit
            ));
        }
    }
}

#[tokio::test]
async fn body_facade_and_exact_upload_reservations_use_one_account() {
    let mut session = facade_body_session(b"abcdefgh", 1).await;
    let mut budget = BodyScanBudget::default();
    let limits = VmLimits::default();
    let runtime = DslRuntime {
        max_body_scan_passes: 1,
        ..Default::default()
    };

    budget.reserve_exact(4, 8, 1).unwrap();
    budget.refund_exact(2);
    let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
    assert_eq!(ops.read_text_range(0, 3).await.unwrap(), "abc");
    budget.reserve_exact(3, 8, 1).unwrap();
    let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
    assert!(matches!(
        ops.read_text_range(0, 1).await,
        Err(RuntimeError::BodyAccess { .. })
    ));
}

#[tokio::test]
async fn body_facade_factory_failure_keeps_the_scan_reservation() {
    use crate::request_body::BodyReadError;
    use crate::request_body::{BodyOperationError, Md5Scanner};

    let mut session = facade_body_session(b"abc", 1).await;
    let mut budget = BodyScanBudget::default();
    let limits = VmLimits::default();
    let runtime = DslRuntime {
        max_body_scan_passes: 1,
        ..Default::default()
    };
    let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
    let result = ops
        .scan_with::<Md5Scanner, _>(async { Err(BodyOperationError::Body(BodyReadError::Incomplete)) })
        .await;
    assert!(matches!(result, Err(RuntimeError::BodyAccess { .. })));
    assert_eq!(budget.reserve(3, 1).unwrap(), 0);
}

#[tokio::test]
async fn body_facade_cancelled_factory_does_not_refund_pending_work() {
    use crate::request_body::{BodyOperationError, Md5Scanner};
    use futures::FutureExt;

    let mut session = facade_body_session(b"abc", 1).await;
    let mut budget = BodyScanBudget::default();
    let limits = VmLimits::default();
    let runtime = DslRuntime {
        max_body_scan_passes: 1,
        ..Default::default()
    };
    let mut ops = DslBodyOps::new(&mut session, &mut budget, &limits, &runtime);
    let pending = std::future::pending::<Result<Md5Scanner, BodyOperationError>>();
    assert!(ops.scan_with(pending).now_or_never().is_none());
    assert_eq!(budget.reserve(3, 1).unwrap(), 0);
}

fn available_body_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .withf(|known_bodyless| !known_bodyless)
        .return_const(Ok(0));
    session
}

#[test]
fn body_append_requires_exact_string_argument() {
    let error = run_expect_runtime_err("req.body.append(nil)", available_body_session());
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            expected: "Str",
            got: "Nil",
            operation: "body.append"
        }
    ));
}

#[test]
fn body_replace_requires_exact_string_arguments() {
    let error = run_expect_runtime_err("req.body.replace(1, false)", available_body_session());
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            expected: "Str",
            got: "Bool",
            operation: "body.replace"
        }
    ));
}

#[test]
fn body_replace_rejects_dynamic_empty_search_before_scanning() {
    let mut session = available_body_session();
    session.expect_ctx().times(0);
    session.expect_request_body_replace_literal().times(0);

    let error = run_expect_runtime_err(
        r#"
            let search = ""
            req.body.replace(search, "replacement")
        "#,
        session,
    );
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            expected: "non-empty Str",
            got: "Str",
            operation: "body.replace"
        }
    ));
}

#[test]
fn body_digest_methods_require_body_receiver() {
    for (source, operation) in [
        (r#""abc".md5()"#, "body.md5"),
        (r#""abc".hmac_sha256("secret")"#, "body.hmac_sha256"),
    ] {
        let error = run_expect_runtime_err(source, MockPluginSession::new());
        assert!(matches!(
            error,
            RuntimeError::TypeError {
                expected: "Body",
                got: "Str",
                operation: actual
            } if actual == operation
        ));
    }
}

fn run_body_json_script(
    source: &str,
    body: &[u8],
    memory_limit: usize,
    max_body_scan_passes: u8,
) -> Result<PluginRunningResult, RuntimeError> {
    run_body_script_with_limits(source, body, memory_limit, max_body_scan_passes, VmLimits::default())
}

fn run_body_script_with_limits(
    source: &str,
    body: &[u8],
    memory_limit: usize,
    max_body_scan_passes: u8,
    limits: VmLimits,
) -> Result<PluginRunningResult, RuntimeError> {
    use bytes::Bytes;
    use pingora_core::protocols::http::RequestBodyBuffer;

    let vm = make_vm_with_limits(source, limits);
    let mut log = PluginLog::new("test");
    let runtime = DslRuntime {
        max_body_scan_passes,
        ..Default::default()
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let cap = u64::try_from(body.len()).unwrap();
            let (mut storage, view) = crate::request_body::BodyStorage::new(memory_limit, cap);
            storage.write(&Bytes::copy_from_slice(body)).await.unwrap();
            storage.finish().await.unwrap();

            let mut ctx = crate::EdgionHttpContext::new();
            ctx.request_body_config.max_body_size_bytes = cap;
            ctx.request_body.install(view);

            let mut session = MockPluginSession::new();
            session.expect_declares_body().times(1..=8).return_const(Some(true));
            session
                .expect_request_body_available_len()
                .withf(|known_bodyless| !known_bodyless)
                .times(1..=8)
                .return_const(Ok(cap));
            session.expect_ctx().times(0..=16).return_const(ctx);
            vm.execute(&mut session as &mut dyn PluginSession, &mut log, &runtime)
                .await
        })
}

#[test]
fn body_text_reads_bound_lossy_expansion_in_memory_and_spill() {
    for body in [&[0xff][..], &[0xe2, 0x82][..]] {
        for source in [
            "let text = req.body.read(0, 2)",
            "let text = req.body.read(0, req.body.byte_len())",
        ] {
            for memory_limit in [16, 0] {
                let result = run_body_script_with_limits(
                    source,
                    body,
                    memory_limit,
                    1,
                    VmLimits {
                        max_string_len: 2,
                        ..Default::default()
                    },
                );
                assert!(matches!(result, Err(RuntimeError::StringTooLong { len: 3, limit: 2 })));

                let success = run_body_script_with_limits(
                    source,
                    body,
                    memory_limit,
                    1,
                    VmLimits {
                        max_string_len: 3,
                        ..Default::default()
                    },
                );
                assert_eq!(success.unwrap(), PluginRunningResult::GoodNext);
            }
        }
    }
}

#[test]
fn body_text_read_ranges_preserve_raw_offsets_and_budget_refunds() {
    let source = r#"
        if req.body.read(0, 1) != "a" { return deny(400, "prefix") }
        if req.body.read(1, 3) != "€" { return deny(400, "scalar") }
        if req.body.read(4, 3) != "z" { return deny(400, "tail") }
        if req.body.read(5, 0) != "" { return deny(400, "empty") }
    "#;
    for memory_limit in [16, 1] {
        assert_eq!(
            run_body_script_with_limits(
                source,
                "a€z".as_bytes(),
                memory_limit,
                1,
                VmLimits {
                    max_string_len: 3,
                    ..Default::default()
                },
            )
            .unwrap(),
            PluginRunningResult::GoodNext
        );
    }
}

#[test]
fn body_text_read_length_error_precedes_exhausted_scan_budget() {
    let body = vec![b'x'; 64];
    for memory_limit in [128, 1] {
        let limits = VmLimits {
            max_string_len: 32,
            ..Default::default()
        };
        let range = run_body_script_with_limits(
            "let digest = req.body.md5() let text = req.body.read(0, 64)",
            &body,
            memory_limit,
            1,
            limits.clone(),
        );
        assert!(matches!(range, Err(RuntimeError::StringTooLong { len: 64, limit: 32 })));
        let admitted = run_body_script_with_limits(
            "let digest = req.body.md5() let text = req.body.read(0, 32)",
            &body,
            memory_limit,
            1,
            limits.clone(),
        );
        assert!(matches!(admitted, Err(RuntimeError::BodyAccess { .. })));
        let bounded = run_body_script_with_limits("let text = req.body.read(0, 64)", &body, memory_limit, 1, limits);
        assert!(matches!(
            bounded,
            Err(RuntimeError::StringTooLong { len: 64, limit: 32 })
        ));
    }
}

#[test]
fn body_json_select_reads_multiple_paths_with_one_spilled_scan() {
    let body = br#"{"user":{"id":7,"name":"alice"},"enabled":true}"#;
    let source = r#"
        let body = req.body
        let values = body.json_select({
            "id": "$.user.id",
            "name": "$.user.name",
            "enabled": "$.enabled"
        })
        if values["id"] != 7 || values["name"] != "alice" || values["enabled"] != true {
            return deny(400, "unexpected selection")
        }
    "#;

    assert_eq!(
        run_body_json_script(source, body, 1, 1).unwrap(),
        PluginRunningResult::GoodNext
    );
}

#[test]
fn repeated_body_json_get_exhausts_the_same_one_pass_budget() {
    let body = br#"{"a":1,"b":2}"#;
    let source = r#"
        let a = req.body.json_get("$.a")
        let b = req.body.json_get("$.b")
    "#;

    assert!(matches!(
        run_body_json_script(source, body, 1, 1),
        Err(RuntimeError::BodyAccess { .. })
    ));
}

#[test]
fn repeated_body_sha256_reuses_one_spilled_scan() {
    let body = b"one physical scan";
    let source = r#"
        let first = req.body.sha256()
        let second = req.body.sha256()
        if first != second {
            return deny(400, "digest cache mismatch")
        }
    "#;

    assert_eq!(
        run_body_json_script(source, body, 1, 1).unwrap(),
        PluginRunningResult::GoodNext
    );
}

#[test]
fn body_md5_and_hmac_sha256_match_raw_byte_vectors_in_memory_and_spill() {
    let source = r#"
        if req.body.md5() != "900150983cd24fb0d6963f7d28e17f72" {
            return deny(400, "MD5 mismatch")
        }
        if req.body.hmac_sha256("secret") != "9946dad4e00e913fc8be8e5d3f7e110a4a9e832f83fb09c345285d78638d8a0e" {
            return deny(400, "HMAC mismatch")
        }
    "#;
    for memory_limit in [4, 1] {
        assert_eq!(
            run_body_json_script(source, b"abc", memory_limit, 2).unwrap(),
            PluginRunningResult::GoodNext
        );
    }

    let binary_source = r#"
        if req.body.md5() != "1be512fdd3d66cd1acc59d4c45a9e7df" {
            return deny(400, "binary MD5 mismatch")
        }
        if req.body.hmac_sha256("secret") != "20676f860da7de07e1311f2907682d64a4491ca78d7247f8df8dd158ec2666d1" {
            return deny(400, "binary HMAC mismatch")
        }
    "#;
    for memory_limit in [5, 1] {
        assert_eq!(
            run_body_json_script(binary_source, &[b'a', 0, 0xff, b'z'], memory_limit, 2).unwrap(),
            PluginRunningResult::GoodNext
        );
    }
}

#[test]
fn body_md5_and_hmac_each_consume_uncached_scan_budget() {
    for source in [
        r#"
            let first = req.body.md5()
            let second = req.body.md5()
        "#,
        r#"
            let first = req.body.hmac_sha256("secret")
            let second = req.body.hmac_sha256("secret")
        "#,
    ] {
        assert!(matches!(
            run_body_json_script(source, b"abc", 1, 1),
            Err(RuntimeError::BodyAccess { .. })
        ));
    }
}

#[test]
fn body_digest_fixed_output_limits_fail_before_scan_without_key_leak() {
    for (source, max_string_len, expected_len) in [
        ("req.body.sha256()", 63, 64),
        ("req.body.md5()", 31, 32),
        (r#"req.body.hmac_sha256("sentinel-secret")"#, 63, 64),
    ] {
        let error = run_body_script_with_limits(
            source,
            b"abc",
            1,
            0,
            VmLimits {
                max_string_len,
                ..VmLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::StringTooLong { len, limit }
                if len == expected_len && limit == max_string_len
        ));
        assert!(!error.to_string().contains("sentinel-secret"));
    }
}

#[test]
fn body_hmac_requires_exact_string_key_without_value_leak() {
    let error = run_body_json_script("req.body.hmac_sha256(7)", b"abc", 1, 1).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            expected: "Str",
            got: "Int",
            operation: "body.hmac_sha256"
        }
    ));
    assert!(!error.to_string().contains('7'));
}

#[test]
fn body_hmac_scan_failure_does_not_leak_key() {
    let source = r#"
        let consume = req.body.md5()
        let mac = req.body.hmac_sha256("sentinel-secret")
    "#;
    let error = run_body_json_script(source, b"abc", 1, 1).unwrap_err();
    assert!(matches!(error, RuntimeError::BodyAccess { .. }));
    assert!(!error.to_string().contains("sentinel-secret"));
}
