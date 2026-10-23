use async_trait::async_trait;
use bytes::Bytes;
use std::io::Write as _;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use crate::backends::{
    get_global_backend_tls_policy_store, get_service_discovery_coordinator, validate_endpoint_backend_addr,
    validate_endpoint_in_route,
};
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::session_adapter::key_value;
use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::routes::http::match_unit::CompiledBackend;
use crate::{DirectEndpointPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DirectEndpointConfig, UpstreamFallback};

const MISSING_TARGET_ENDPOINT_BODY: Bytes = Bytes::from_static(b"Missing target endpoint");
const INVALID_TARGET_ENDPOINT_BODY: Bytes = Bytes::from_static(b"Invalid target endpoint");
const TARGET_ENDPOINT_NOT_ALLOWED_BODY: Bytes = Bytes::from_static(b"Target endpoint not allowed");

pub struct DirectEndpoint {
    config: DirectEndpointConfig,
}

impl DirectEndpoint {
    pub fn new(config: &DirectEndpointConfig) -> Self {
        let mut validated = config.clone();
        validated.validate();
        Self { config: validated }
    }

    /// Resolve the override against one immutable discovery generation.
    fn resolve(
        &self,
        raw_value: &str,
        backends: &[CompiledBackend],
        allow_loopback_upstream: bool,
    ) -> Result<DirectEndpointPreset, ResolutionError> {
        // 1. Apply regex extraction if configured
        let endpoint_str = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str(),
                    None => return Err(ResolutionError::Missing),
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 2. Parse IP (support "ip" or "ip:port" format)
        let (target_ip, extracted_port) = parse_endpoint(endpoint_str).map_err(|_| ResolutionError::Invalid)?;

        // 3. Security: validate against route's backend_ref endpoints
        let discovery = get_service_discovery_coordinator().load_generation();
        let policies = get_global_backend_tls_policy_store().snapshot();
        let endpoint = validate_endpoint_in_route(
            target_ip,
            extracted_port.or(self.config.port),
            backends,
            &discovery,
            &policies,
            self.config.inherit_tls,
        )
        .map_err(|_| ResolutionError::NotAllowed)?;
        let addr = endpoint.addr;
        if validate_endpoint_backend_addr(
            &pingora_core::protocols::l4::socket::SocketAddr::Inet(addr),
            allow_loopback_upstream,
        )
        .is_err()
        {
            return Err(ResolutionError::NotAllowed);
        }

        Ok(DirectEndpointPreset {
            addr,
            tls_policy: endpoint.tls_policy,
            backend_ref_idx: endpoint.backend_ref_idx,
            app_protocol: endpoint.app_protocol,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolutionError {
    Missing,
    Invalid,
    NotAllowed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InvalidEndpoint;

fn format_endpoint(addr: SocketAddr, buffer: &mut [u8; 64]) -> &str {
    let mut remaining = buffer.as_mut_slice();
    write!(&mut remaining, "{addr}").expect("numeric SocketAddr text including IPv6 scope and port fits in 58 bytes");
    let len = 64 - remaining.len();
    std::str::from_utf8(&buffer[..len]).expect("numeric SocketAddr formatting produces only ASCII")
}

#[async_trait]
impl RequestFilter for DirectEndpoint {
    fn name(&self) -> &str {
        "DirectEndpoint"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw endpoint value via KeyGet
        let raw_value = match key_value(session, &self.config.from) {
            Some(v) if !v.is_empty() => v,
            _ => match self.config.on_missing {
                UpstreamFallback::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-missing");
                    log.push("no value fallback");
                    return PluginRunningResult::GoodNext;
                }
                UpstreamFallback::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-missing");
                    log.push("no value 400");
                    return PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some(MISSING_TARGET_ENDPOINT_BODY),
                    };
                }
            },
        };

        // 2. Get route context
        let ctx = session.ctx();
        let backends = match ctx.route_unit.as_ref() {
            Some(unit) if !unit.backends.is_empty() => unit.backends.as_ref(),
            _ => {
                log.push("no route refs");
                return PluginRunningResult::GoodNext;
            }
        };

        // 3. Resolve endpoint info
        match self.resolve(&raw_value, backends, ctx.allow_loopback_upstream) {
            Ok(info) => {
                let addr = info.addr;
                match session.set_upstream_jump(UpstreamJumpTarget::Direct(info)) {
                    SetJumpOutcome::Set => {
                        let mut buffer = [0u8; 64];
                        let addr_str = format_endpoint(addr, &mut buffer);
                        log.push_fmt("endpoint {}", la![sensitive(addr_str)]);
                        session.record_outcome(DyeOutcome::Success, addr_str);
                    }
                    SetJumpOutcome::Rejected => {
                        log.push("jump locked, rejected");
                        return PluginRunningResult::GoodNext;
                    }
                }

                PluginRunningResult::GoodNext
            }
            Err(ResolutionError::Missing) => {
                log.push("endpoint miss");
                match self.config.on_missing {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some(MISSING_TARGET_ENDPOINT_BODY),
                        }
                    }
                }
            }
            Err(error @ (ResolutionError::Invalid | ResolutionError::NotAllowed)) => {
                log.push("endpoint invalid");
                match self.config.on_invalid {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-invalid");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-invalid");
                        let (status, body) = match error {
                            ResolutionError::Invalid => (400, INVALID_TARGET_ENDPOINT_BODY),
                            ResolutionError::NotAllowed => (403, TARGET_ENDPOINT_NOT_ALLOWED_BODY),
                            ResolutionError::Missing => unreachable!("handled by the preceding match arm"),
                        };
                        PluginRunningResult::ErrResponse {
                            status,
                            body: Some(body),
                        }
                    }
                }
            }
        }
    }
}

/// Parse endpoint string into (IpAddr, optional port)
fn parse_endpoint(s: &str) -> Result<(IpAddr, Option<u16>), InvalidEndpoint> {
    if let Some(bracketed) = s.strip_prefix('[') {
        let (ip, suffix) = bracketed.split_once(']').ok_or(InvalidEndpoint)?;
        let ip = ip.parse::<Ipv6Addr>().map_err(|_| InvalidEndpoint)?;
        let port = match suffix {
            "" => None,
            _ => {
                let port = suffix
                    .strip_prefix(':')
                    .filter(|port| !port.is_empty())
                    .ok_or(InvalidEndpoint)?;
                Some(port.parse::<u16>().map_err(|_| InvalidEndpoint)?)
            }
        };
        return Ok((IpAddr::V6(ip), port));
    }

    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok((ip, None));
    }
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return Ok((addr.ip(), Some(addr.port())));
    }

    Err(InvalidEndpoint)
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/direct_endpoint/plugin_test.rs"]
mod tests;
