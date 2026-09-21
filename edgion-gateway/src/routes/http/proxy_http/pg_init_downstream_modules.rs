use super::EdgionHttpProxy;
use pingora_core::modules::http::compression::ResponseCompressionBuilder;
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;

/// Compression level used when downstream compression is enabled. There is no
/// operator-facing level knob; pingora treats level 0 as "module present but
/// disabled", which is what a `false` config value must produce so the
/// final per-response adjust in `pg_response_filter` has a ctx to act on.
pub(crate) const DEFAULT_COMPRESSION_LEVEL: u32 = 6;

#[inline]
fn register_downstream_modules(modules: &mut HttpModules, compression_level: u32) {
    modules.add_module(ResponseCompressionBuilder::enable(compression_level));
    modules.add_module(Box::new(GrpcWeb));
}

#[inline]
pub fn init_downstream_modules(_edgion_http: &EdgionHttpProxy, modules: &mut HttpModules) {
    // Always register the module. The early request hook applies the captured
    // switch before Accept-Encoding parsing; the response hook narrows encoding.
    register_downstream_modules(modules, DEFAULT_COMPRESSION_LEVEL);
}

#[cfg(test)]
mod tests {
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
}
