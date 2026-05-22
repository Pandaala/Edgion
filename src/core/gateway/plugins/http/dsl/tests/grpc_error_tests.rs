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
fn test_return_grpc_error_out_of_range_code() {
    let source = r#"return grpc_error(99, "bad code")"#;
    let session = MockPluginSession::new();
    let result = run(source, session);
    // Out of range (0-16) → defaults to 13 (INTERNAL)
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 13,
            message: Some("bad code".to_string()),
        }
    );
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
