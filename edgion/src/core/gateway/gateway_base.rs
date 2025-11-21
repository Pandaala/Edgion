use std::sync::Arc;
use std::time::SystemTime;
use kube::ResourceExt;
use pingora_core::prelude::Opt;
use pingora_core::server::Server;
use pingora_proxy::http_proxy_service;
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::types::{EdgionGatewayConfig, Gateway, GatewayClass};
use anyhow::Result;
use crate::core::tls::tls_pingora::TlsCallback;

pub struct GatewayBase {
    gateway_class: GatewayClass,
    edgion_gateway_config: EdgionGatewayConfig,
    gateways: Vec<Gateway>,
}

impl GatewayBase {
    pub fn new(gateway_class: GatewayClass, edgion_gateway_config: EdgionGatewayConfig, gateways: Vec<Gateway>) -> Self {
        Self {
            gateway_class,
            edgion_gateway_config,
            gateways,
        }
    }

    pub fn bootstrap(&self) {
        tracing::info!("Bootstrapping gateways");
    }

    pub fn run_forever(&self) -> Result<()> {

        // create pingora server
        let mut pingora_server = Server::new(Some(Opt::default()))?;
        pingora_server.bootstrap();

        for gateway in self.gateways.iter() {
            if let Some(listeners) = &gateway.spec.listeners {
                for listener in listeners {

                    let host = listener.hostname.as_deref().unwrap_or("0.0.0.0");
                    let port = listener.port;
                    let addr = format!("{}:{}", host, port);

                    let enable_tls = {
                        if listener.tls.is_some() {
                            true
                        } else {
                            if listener.port == 443 || listener.port == 8443 {
                                true
                            } else {
                                false
                            }
                        }
                    };

                    let edgion_http = EdgionHttp{
                        gateway_class_name: self.gateway_class.metadata.name.clone(),
                        gateway_namespace: gateway.metadata.namespace.clone(),
                        gateway_name: gateway.name_any(),
                        server_start_time: SystemTime::now(),
                        server_header_opts: Default::default(),
                        ctx_cnt: Arc::new(Default::default()),
                    };

                    let mut http_service = http_proxy_service(&pingora_server.configuration, edgion_http);

                    if enable_tls {
                        let tls_settings = TlsCallback::new_tls_settings_with_callback()?;
                        http_service.add_tls_with_settings(&addr, None, tls_settings);
                    } else {
                        http_service.add_tcp(&addr);
                    }

                    pingora_server.add_service(http_service);
                }
            }
        }

        Ok(())
    }
}