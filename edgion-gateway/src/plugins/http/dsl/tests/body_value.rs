use super::*;

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
