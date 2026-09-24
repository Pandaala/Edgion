//! Tests for return grpc_error() — returning gRPC status codes from DSL.

use super::*;

#[test]
fn test_return_grpc_error_basic() {
    let source = r#"return grpc_error(7, "permission denied")"#;
    let session = MockPluginSession::new();
    let result = run(source, session);
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: Some("permission denied".to_string()),
        }
    );
}

#[test]
fn test_return_grpc_error_nil_message() {
    let result = run("return grpc_error(7, nil)", MockPluginSession::new());

    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: None,
        }
    );
}

#[test]
fn test_return_grpc_error_out_of_range_code() {
    for code in [-1, 17] {
        let source = format!(r#"return grpc_error({code}, "bad code")"#);
        let result = run(&source, MockPluginSession::new());

        // Codes outside 0..=16 default to INTERNAL while preserving the message.
        assert_eq!(
            result,
            PluginRunningResult::ErrGrpcError {
                grpc_status: 13,
                message: Some("bad code".to_string()),
            }
        );
    }
}

#[test]
fn test_return_grpc_error_conditional() {
    let source = r#"
        if req.path.starts_with("/admin") {
            return grpc_error(7, "PERMISSION_DENIED: admin access blocked")
        }
        return next()
    "#;
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/admin/users".to_string());
    let result = run(source, session);
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: Some("PERMISSION_DENIED: admin access blocked".to_string()),
        }
    );
}
