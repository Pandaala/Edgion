use super::EdgionHttp;
use pingora_core::modules::http::compression::ResponseCompressionBuilder;
use pingora_core::modules::http::grpc_web::GrpcWeb;
use pingora_core::modules::http::HttpModules;

#[inline]
pub fn init_downstream_modules(edgion_http: &EdgionHttp, modules: &mut HttpModules) {
    // Configure downstream compression based on global config (default: disabled)
    let enable_compression = edgion_http
        .edgion_gateway_config
        .spec
        .server
        .as_ref()
        .map(|s| s.enable_compression)
        .unwrap_or(false);

    if !enable_compression {
        // Explicitly disable compression
        modules.add_module(ResponseCompressionBuilder::enable(0));
    }

    // Only add GrpcWeb module if HTTP/2 is enabled
    // gRPC-Web requires HTTP/2 support
    if edgion_http.enable_http2 {
        modules.add_module(Box::new(GrpcWeb));
        let gw_name = edgion_http.gateway_infos.first().map(|gi| gi.name.as_str()).unwrap_or("unknown");
        tracing::info!(gateway=%gw_name, listener=%edgion_http.listener.name, "GrpcWeb module enabled");
    }
}
