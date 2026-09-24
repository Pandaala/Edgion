use super::{register_downstream_modules, DEFAULT_COMPRESSION_LEVEL};
use pingora_core::modules::http::compression::ResponseCompression;
use pingora_core::modules::http::grpc_web::GrpcWebBridge;
use pingora_core::modules::http::HttpModules;

#[test]
fn modules_with_disabled_compression_include_grpc_web_bridge() {
    let mut modules = HttpModules::new();
    register_downstream_modules(&mut modules, 0);

    let ctx = modules.build_ctx();
    assert!(ctx.get::<GrpcWebBridge>().is_some());
    assert!(!ctx
        .get::<ResponseCompression>()
        .expect("response compression module")
        .is_enabled());
}

#[test]
fn modules_with_enabled_compression_include_grpc_web_bridge() {
    let mut modules = HttpModules::new();
    register_downstream_modules(&mut modules, DEFAULT_COMPRESSION_LEVEL);

    let ctx = modules.build_ctx();
    assert!(ctx.get::<GrpcWebBridge>().is_some());
    assert!(ctx
        .get::<ResponseCompression>()
        .expect("response compression module")
        .is_enabled());
}
