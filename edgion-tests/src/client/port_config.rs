// Port configuration loaded from ports.json.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
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
    pub tcp_bare: Option<u16>,
    pub tcp_conn_limit: Option<u16>,
    pub tls: Option<u16>,
    pub udp: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    Http,
    Https,
    Grpc,
    GrpcTls,
    Tcp,
    Tls,
    Udp,
}

impl PortConfig {
    /// Load the port configuration from ports.json.
    pub fn load() -> Result<Self, String> {
        let possible_paths = [
            "edgion-tests/integration/conf/ports.json",
            "../conf/ports.json",
            "conf/ports.json",
        ];

        for path in &possible_paths {
            if Path::new(path).exists() {
                let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path, e))?;
                return serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path, e));
            }
        }

        Err("Could not find ports.json in any expected location".to_string())
    }

    /// Look up an exact suite key. Missing keys are configuration errors.
    pub fn get_ports(&self, suite: &str) -> Result<&SuitePorts, String> {
        self.suites
            .get(suite)
            .ok_or_else(|| format!("ports.json has no entry for suite `{suite}`"))
    }

    pub fn require_port(&self, suite: &str, kind: PortKind) -> Result<u16, String> {
        let ports = self.get_ports(suite)?;
        let (field, value) = match kind {
            PortKind::Http => ("http", ports.http),
            PortKind::Https => ("https", ports.https),
            PortKind::Grpc => ("grpc", ports.grpc),
            PortKind::GrpcTls => ("grpc_tls", ports.grpc_tls),
            PortKind::Tcp => ("tcp", ports.tcp),
            PortKind::Tls => ("tls", ports.tls),
            PortKind::Udp => ("udp", ports.udp),
        };
        value.ok_or_else(|| format!("ports.json suite `{suite}` is missing required `{field}` port"))
    }
}

/// Map a legacy command alias to its canonical suite name.
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
        _ => "http", // default: http suite
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_command_to_suite() {
        assert_eq!(command_to_suite("http"), "http");
        assert_eq!(command_to_suite("http-match"), "http-match");
        assert_eq!(command_to_suite("grpc-tls"), "grpc-tls");
    }

    fn rust_files_under(path: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("suite source directory must be readable") {
            let path = entry.expect("suite source entry must be readable").path();
            if path.is_dir() {
                rust_files_under(&path, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    #[test]
    fn unknown_suite_and_missing_protocol_fail_closed() {
        let config: PortConfig = serde_json::from_str(r#"{"current_max": 1, "suites": {"http-only": {"http": 8080}}}"#)
            .expect("fixture must parse");

        assert!(config.get_ports("missing").is_err());
        assert!(config.require_port("http-only", PortKind::Grpc).is_err());
        assert_eq!(config.require_port("http-only", PortKind::Http).unwrap(), 8080);
    }

    #[test]
    fn every_declared_suite_port_key_has_an_http_mapping() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config: PortConfig = serde_json::from_str(
            &fs::read_to_string(manifest_dir.join("integration/conf/ports.json")).expect("ports.json must be readable"),
        )
        .expect("ports.json must parse");
        let mut files = Vec::new();
        rust_files_under(&manifest_dir.join("src/client/suites"), &mut files);

        let mut keys = Vec::new();
        for file in files {
            let source = fs::read_to_string(&file).expect("suite source must be readable");
            for (offset, _) in source.match_indices("fn port_key") {
                let body = &source[offset..source.len().min(offset + 500)];
                if let Some(start) = body.find("Some(\"") {
                    let value = &body[start + "Some(\"".len()..];
                    if let Some(end) = value.find('"') {
                        keys.push((value[..end].to_string(), file.clone()));
                    }
                }
            }
        }

        assert!(keys.len() > 30, "expected to inspect the suite port-key inventory");
        let failures: Vec<_> = keys
            .into_iter()
            .filter_map(|(key, file)| {
                config
                    .require_port(&key, PortKind::Http)
                    .err()
                    .map(|error| format!("{}: {error}", file.display()))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "invalid suite port_key declarations:\n{}",
            failures.join("\n")
        );
    }
}
