use super::EdgionHttpProxy;
use pingora_core::modules::http::compression::ResponseCompressionBuilder;
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;

/// Compression level used when downstream compression is enabled. There is no
/// operator-facing level knob; pingora treats level 0 as "module present but
/// disabled", which is what a `false` config value must produce so the
/// per-response adjust in `pg_upstream_response_filter` has a ctx to act on.
pub(crate) const DEFAULT_COMPRESSION_LEVEL: u32 = 6;

#[inline]
pub fn init_downstream_modules(edgion_http: &EdgionHttpProxy, modules: &mut HttpModules) {
    let enable_compression = edgion_http
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|s| s.enable_compression)
        .unwrap_or(false);

    // Always install the module: enabled at the default level or explicitly
    // disabled (level 0). Installing it unconditionally keeps a
    // ResponseCompressionCtx in every session so per-response adjustment is
    // uniform.
    let level = if enable_compression {
        DEFAULT_COMPRESSION_LEVEL
    } else {
        0
    };
    modules.add_module(ResponseCompressionBuilder::enable(level));

    // Only add GrpcWeb module if HTTP/2 is enabled
    if edgion_http.enable_http2 {
        modules.add_module(Box::new(GrpcWeb));
    }
}
