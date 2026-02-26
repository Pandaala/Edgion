// Port config
//  ports.json Port config

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize, Debug)]
pub struct PortConfig {
    pub current_max: u16,
    pub suites: HashMap<String, SuitePorts>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct SuitePorts {
    pub http: Option<u16>,
    pub https: Option<u16>,
    pub grpc: Option<u16>,
    pub grpc_tls: Option<u16>,
    pub tcp: Option<u16>,
    pub tcp_filtered: Option<u16>,
    pub udp: Option<u16>,
}

impl PortConfig {
    ///  ports.json Port config
    pub fn load() -> Result<Self, String> {
        // 
        let possible_paths = ["examples/test/conf/ports.json", "../conf/ports.json", "conf/ports.json"];

        for path in &possible_paths {
            if Path::new(path).exists() {
                let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
                return serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path, e));
            }
        }

        Err("Could not find ports.json in any expected location".to_string())
    }

    /// Port config
    pub fn get_ports(&self, suite: &str) -> SuitePorts {
        self.suites.get(suite).cloned().unwrap_or_default()
    }
}

/// suite name
pub fn command_to_suite(command: &str) -> &str {
    match command {
        "http" => "http",
        "https" => "https",
        "http-match" | "httpmatch" => "http-match",
        "http-security" | "httpsecurity" => "http-security",
        "http-redirect" | "httpredirect" => "http-redirect",
        "grpc" => "grpc",
        "grpc-match" | "grpcmatch" => "grpc-match",
        "grpc-tls" | "grpctls" => "grpc-tls",
        "websocket" => "websocket",
        "tcp" => "tcp",
        "udp" => "udp",
        "mtls" => "mtls",
        "cipher" => "cipher",
        "lb-rr" | "lbrr" | "lb-roundrobin" => "lb-roundrobin",
        "lb-ch" | "lbch" | "lb-consistenthash" => "lb-consistenthash",
        "weighted-backend" | "weightedbackend" => "weighted-backend",
        "timeout" => "timeout",
        "security" => "security",
        "real-ip" | "realip" => "real-ip",
        "backend-tls" | "backendtls" => "backend-tls",
        "plugin-logs" | "pluginlogs" => "plugin-logs",
        "stream-plugins" | "streamplugins" => "stream-plugins",
        _ => "http", //  http port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_to_suite() {
        assert_eq!(command_to_suite("http"), "http");
        assert_eq!(command_to_suite("http-match"), "http-match");
        assert_eq!(command_to_suite("grpc-tls"), "grpc-tls");
    }
}
