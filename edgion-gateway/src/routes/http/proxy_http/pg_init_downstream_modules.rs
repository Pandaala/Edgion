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
pub fn init_downstream_modules(edgion_http: &EdgionHttpProxy, modules: &mut HttpModules) {
    let enable_compression = edgion_http
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|server| server.enable_compression)
        .unwrap_or(false);

    // An enabled initial level is required for Pingora's request filter to
    // capture Accept-Encoding. The final response hook, shared by origin and
    // cache-hit paths, always narrows this to level 0 or the default before the
    // response module selects an encoder.
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
