use async_trait::async_trait;
use std::net::IpAddr;

use crate::core::backends::validate_endpoint_in_route;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{DirectEndpointConfig, DirectEndpointOnInvalid, DirectEndpointOnMissing};
use crate::types::{DirectEndpointPreset, HTTPBackendRef};

pub struct DirectEndpoint {
    name: String,
    config: DirectEndpointConfig,
}

impl DirectEndpoint {
    pub fn new(config: &DirectEndpointConfig) -> Self {
        let mut validated = config.clone();
        validated.validate();
        Self {
            name: "DirectEndpoint".to_string(),
            config: validated,
        }
    }

    /// Pure function to resolve endpoint info from raw value + context
    fn resolve(
        &self,
        raw_value: String,
        backend_refs: &[HTTPBackendRef],
        route_ns: &str,
    ) -> Result<DirectEndpointPreset, ResolutionError> {
        // 1. Apply regex extraction if configured
        let endpoint_str = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => return Err(ResolutionError::Missing("Regex miss".to_string())),
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 2. Parse IP (support "ip" or "ip:port" format)
        let (target_ip, extracted_port) =
            parse_endpoint(&endpoint_str).map_err(|e| ResolutionError::Invalid(format!("ParseErr {}", e)))?;

        // 3. Security: validate against route's backend_ref endpoints
        let (backend_idx, effective_port) =
            match validate_endpoint_in_route(&target_ip, extracted_port.or(self.config.port), backend_refs, route_ns) {
                Ok(result) => result,
                Err(reason) => {
                    tracing::warn!(
                        target_ip = %target_ip,
                        reason = %reason,
                        "DirectEndpoint: endpoint validation failed"
                    );
                    return Err(ResolutionError::Invalid(reason)); // Becomes Deny
                }
            };

        // 4. Determine TLS config
        let (use_tls, sni) = if self.config.inherit_tls {
            extract_tls_from_backend_ref(&backend_refs[backend_idx])
        } else {
            (false, String::new())
        };

        let addr = std::net::SocketAddr::new(target_ip, effective_port);
        Ok(DirectEndpointPreset {
            addr,
            use_tls,
            sni,
            backend_ref_idx: backend_idx,
        })
    }
}

enum ResolutionError {
    Missing(String),
    Invalid(String),
}

#[async_trait]
impl RequestFilter for DirectEndpoint {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("Skip gRPC; ");
            return PluginRunningResult::GoodNext;
        }

        // 1. Check config validation
        if let Some(err) = self.config.get_validation_error() {
            log.push(&format!("CfgErr {}; ", err));
            return PluginRunningResult::GoodNext; // Fail-open on config error
        }

        // 2. Extract raw endpoint value via KeyGet
        let raw_value = match session.key_get(&self.config.from) {
            Some(v) if !v.is_empty() => v,
            _ => match self.config.on_missing {
                DirectEndpointOnMissing::Fallback => {
                    log.push("NoVal fallback; ");
                    return PluginRunningResult::GoodNext;
                }
                DirectEndpointOnMissing::Reject => {
                    log.push("NoVal 400; ");
                    return PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some("Missing target endpoint".to_string()),
                    };
                }
            },
        };

        // 3. Get route context
        let ctx = session.ctx();
        let (backend_refs, route_ns) = match ctx.route_unit.as_ref() {
            Some(unit) if unit.rule.backend_refs.as_ref().is_some_and(|r| !r.is_empty()) => {
                (unit.rule.backend_refs.as_ref().unwrap(), unit.matched_info.rns.as_str())
            }
            _ => {
                log.push("NoRouteOrBkRefs; ");
                return PluginRunningResult::GoodNext;
            }
        };

        // 4. Resolve endpoint info
        match self.resolve(raw_value, backend_refs, route_ns) {
            Ok(info) => {
                let addr_str = format!("{}", info.addr);
                session.set_direct_endpoint(info);
                log.push(&format!("OK {}; ", addr_str));

                if self.config.debug_header {
                    let _ = session.set_response_header("X-Direct-Endpoint", &addr_str);
                }
                PluginRunningResult::GoodNext
            }
            Err(ResolutionError::Missing(msg)) => {
                log.push(&format!("{}; ", msg));
                match self.config.on_missing {
                    DirectEndpointOnMissing::Fallback => PluginRunningResult::GoodNext,
                    DirectEndpointOnMissing::Reject => PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some(format!("DirectEndpoint missing: {}", msg)),
                    },
                }
            }
            Err(ResolutionError::Invalid(msg)) => {
                log.push(&format!("Invalid {}; ", msg));
                match self.config.on_invalid {
                    DirectEndpointOnInvalid::Fallback => PluginRunningResult::GoodNext,
                    DirectEndpointOnInvalid::Reject => {
                        // Use 403 for validation denied, 400 for parse error?
                        // But simplification: 400 or 403.
                        // If parse err -> 400. If deny -> 403.
                        // But here we merged them into Invalid.
                        // msg will contain "Deny" or "ParseErr".
                        let status = if msg.contains("ParseErr") { 400 } else { 403 };
                        PluginRunningResult::ErrResponse {
                            status,
                            body: Some(format!("DirectEndpoint invalid: {}", msg)),
                        }
                    }
                }
            }
        }
    }
}

/// Extract TLS config from a backend_ref's BackendTLSPolicy
fn extract_tls_from_backend_ref(br: &HTTPBackendRef) -> (bool, String) {
    match &br.backend_tls_policy {
        Some(policy) => (true, policy.spec.validation.hostname.clone()),
        None => (false, String::new()),
    }
}

/// Parse endpoint string into (IpAddr, optional port)
fn parse_endpoint(s: &str) -> Result<(IpAddr, Option<u16>), String> {
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return Ok((addr.ip(), Some(addr.port())));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok((ip, None));
    }
    if s.starts_with('[') {
        if let Some(bracket_end) = s.find(']') {
            let ip_str = &s[1..bracket_end];
            let ip: IpAddr = ip_str.parse().map_err(|e| format!("{}", e))?;
            let port = if bracket_end + 1 < s.len() && s.as_bytes()[bracket_end + 1] == b':' {
                Some(s[bracket_end + 2..].parse::<u16>().map_err(|e| format!("{}", e))?)
            } else {
                None
            };
            return Ok((ip, port));
        }
    }
    Err(format!("cannot parse '{}' as endpoint", s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_endpoint() {
        assert_eq!(parse_endpoint("1.2.3.4").unwrap(), ("1.2.3.4".parse().unwrap(), None));
        assert_eq!(
            parse_endpoint("1.2.3.4:80").unwrap(),
            ("1.2.3.4".parse().unwrap(), Some(80))
        );
        assert_eq!(parse_endpoint("[::1]").unwrap(), ("::1".parse().unwrap(), None));
        assert_eq!(
            parse_endpoint("[::1]:8080").unwrap(),
            ("::1".parse().unwrap(), Some(8080))
        );
        assert!(parse_endpoint("invalid").is_err());
    }

    // Since validate_endpoint_in_route calls global store which is hard to mock,
    // we assume validate_endpoint_in_route is tested in validation.rs.
    // However, we can test regex extraction and parsing logic if we mock validation or avoid it.
    // But validate_endpoint_in_route is called inside resolve.
    // So resolve is hard to unit test without mocking global state.
    //
    // But we can test regex extraction logic if we split resolve further.
    // Or we leave it to integration tests.
    //
    // Let's rely on integration tests for full flow, and unit tests for helpers.
}
