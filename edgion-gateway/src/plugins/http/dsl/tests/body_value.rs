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
