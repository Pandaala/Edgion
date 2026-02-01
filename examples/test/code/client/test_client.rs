// Edgion Unified Test Client
// Supports all protocol tests: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

#![allow(dead_code)]
#![allow(unused_imports)]

mod framework;
mod log_analyzer;
pub mod metrics_helper;
mod port_config;
mod reporter;
mod suites;

use anyhow::Result;
use clap::Parser;
use framework::{TestContext, TestRunner};
use reporter::{ConsoleReporter, JsonReporter};
use std::path::PathBuf;
use std::sync::Once;
use std::time::Instant;

static INIT: Once = Once::new();

#[derive(Parser, Debug)]
#[command(name = "test-client")]
#[command(about = "Edgion 统一测试客户端")]
struct Cli {
    /// Resource type (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLS, Security, Plugins)
    #[arg(short = 'r', long = "resource")]
    resource: Option<String>,

    /// sub-item (Match, Backend, Filters, Protocol 等)
    #[arg(short = 'i', long = "item")]
    item: Option<String>,

    /// 使用 Gateway Mode（Passed Gateway 代理测试）
    #[arg(short = 'g', long = "gateway")]
    gateway: bool,

    #[arg(long, default_value = "127.0.0.1")]
    target_host: String,

    #[arg(long, default_value = "30001")]
    http_port: u16,

    #[arg(long, default_value = "30021")]
    grpc_port: u16,

    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    #[arg(long, default_value = "30011")]
    udp_port: u16,

    #[arg(long, default_value = "10443")]
    https_port: u16,

    #[arg(long, default_value = "18443")]
    grpc_https_port: u16,

    #[arg(long, default_value = "5800")]
    admin_port: u16,

    #[arg(long)]
    json: bool,

    #[arg(long, default_value = "test_report.json")]
    json_output: String,

    #[arg(short, long)]
    verbose: bool,

    /// Test phase for dynamic tests (initial or update)
    #[arg(long)]
    phase: Option<String>,

    /// 兼容旧命令：直接指定测试类型
    #[arg(value_name = "COMMAND")]
    legacy_command: Option<String>,
}

/// 解析资源和sub-item，返回 suite 名称
fn resolve_suite(resource: Option<&str>, item: Option<&str>, legacy: Option<&str>) -> String {
    // 优先使用旧的兼容命令
    if let Some(cmd) = legacy {
        return match cmd.to_lowercase().as_str() {
            "http" => "HTTPRoute/Basic".to_string(),
            "http-match" | "httpmatch" => "HTTPRoute/Match".to_string(),
            "http-redirect" | "httpredirect" => "HTTPRoute/Filters/Redirect".to_string(),
            "http-security" | "httpsecurity" => "HTTPRoute/Filters/Security".to_string(),
            "https" => "EdgionTls/https".to_string(),
            "websocket" => "HTTPRoute/Protocol/WebSocket".to_string(),
            "lb-rr" | "lbrr" | "lb-roundrobin" => "HTTPRoute/Backend/LBRoundRobin".to_string(),
            "lb-ch" | "lbch" | "lb-consistenthash" => "HTTPRoute/Backend/LBConsistentHash".to_string(),
            "weighted-backend" | "weightedbackend" => "HTTPRoute/Backend/WeightedBackend".to_string(),
            "timeout" => "HTTPRoute/Backend/Timeout".to_string(),
            "grpc" => "GRPCRoute/Basic".to_string(),
            "grpc-match" | "grpcmatch" => "GRPCRoute/Match".to_string(),
            "grpc-tls" | "grpctls" => "EdgionTls/grpctls".to_string(),
            "tcp" => "TCPRoute/Basic".to_string(),
            "udp" => "UDPRoute/Basic".to_string(),
            "mtls" => "EdgionTls/mTLS".to_string(),
            "security" => "Gateway/Security".to_string(),
            "real-ip" | "realip" => "Gateway/RealIP".to_string(),
            "backend-tls" | "backendtls" => "Gateway/TLS/BackendTLS".to_string(),
            "plugin-logs" | "pluginlogs" => "EdgionPlugins/DebugAccessLog".to_string(),
            "plugin-condition" | "plugincondition" => "EdgionPlugins/PluginCondition".to_string(),
            "all-conditions" | "allconditions" => "EdgionPlugins/PluginCondition/AllConditions".to_string(),
            "jwt-auth" | "jwtauth" => "EdgionPlugins/JwtAuth".to_string(),
            _ => cmd.to_string(),
        };
    }

    // 使用新的 -r/-i 参数
    match (resource, item) {
        (Some(r), Some(i)) => format!("{}/{}", r, i),
        (Some(r), None) => r.to_string(),
        (None, Some(i)) => format!("HTTPRoute/{}", i), // 默认资源为 HTTPRoute
        (None, None) => "all".to_string(),
    }
}

/// 根据 suite 名称获取Port config key
fn suite_to_port_key(suite: &str) -> &str {
    match suite {
        // HTTPRoute
        "HTTPRoute/Basic" | "HTTPRoute" => "HTTPRoute/Basic",
        "HTTPRoute/Match" => "HTTPRoute/Match",
        "HTTPRoute/Backend" | "HTTPRoute/Backend/LBRoundRobin" => "HTTPRoute/Backend/LBRoundRobin",
        "HTTPRoute/Backend/LBConsistentHash" => "HTTPRoute/Backend/LBConsistentHash",
        "HTTPRoute/Backend/WeightedBackend" => "HTTPRoute/Backend/WeightedBackend",
        "HTTPRoute/Backend/Timeout" => "HTTPRoute/Backend/Timeout",
        "HTTPRoute/Filters" | "HTTPRoute/Filters/Redirect" => "HTTPRoute/Filters/Redirect",
        "HTTPRoute/Filters/Security" => "HTTPRoute/Filters/Security",
        "HTTPRoute/Protocol" | "HTTPRoute/Protocol/WebSocket" => "HTTPRoute/Protocol/WebSocket",
        // GRPCRoute
        "GRPCRoute/Basic" | "GRPCRoute" => "GRPCRoute/Basic",
        "GRPCRoute/Match" => "GRPCRoute/Match",
        // TCPRoute
        "TCPRoute/Basic" | "TCPRoute" => "TCPRoute/Basic",
        // UDPRoute
        "UDPRoute/Basic" | "UDPRoute" => "UDPRoute/Basic",
        // Gateway
        "Gateway/Security" | "Gateway" => "Gateway/Security",
        "Gateway/RealIP" => "Gateway/RealIP",
        "Gateway/TLS/BackendTLS" => "Gateway/TLS/BackendTLS",
        "Gateway/TLS/GatewayTLS" => "Gateway/TLS/GatewayTLS",
        "EdgionPlugins/DebugAccessLog" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition" => "EdgionPlugins",
        "EdgionPlugins/PluginCondition/AllConditions" => "EdgionPlugins",
        "EdgionPlugins/JwtAuth" => "EdgionPlugins",
        "Gateway/PortConflict" => "Gateway/PortConflict",
        // EdgionTls
        "EdgionTls" | "EdgionTls/https" => "EdgionTls/https",
        "EdgionTls/grpctls" => "EdgionTls/grpctls",
        "EdgionTls/mTLS" => "EdgionTls/mTLS",
        "EdgionTls/cipher" => "EdgionTls/cipher",
        _ => suite,
    }
}

/// 根据 suite Add test suite到 runner
fn add_suites_for_suite(runner: &mut TestRunner, suite: &str, gateway: bool, phase: Option<&str>) {
    match suite {
        // HTTPRoute 资源
        "HTTPRoute/Basic" | "HTTPRoute" => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
        }
        "HTTPRoute/Match" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpMatchTestSuite));
        }
        "HTTPRoute/Backend" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
            runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
        }
        "HTTPRoute/Backend/LBRoundRobin" => {
            if !gateway {
                eprintln!("Error: LB RoundRobin tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
        }
        "HTTPRoute/Backend/LBConsistentHash" => {
            if !gateway {
                eprintln!("Error: LB ConsistentHash tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
        }
        "HTTPRoute/Backend/WeightedBackend" => {
            if !gateway {
                eprintln!("Error: Weighted backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
        }
        "HTTPRoute/Backend/Timeout" => {
            if !gateway {
                eprintln!("Error: Timeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
        }
        "HTTPRoute/Filters" => {
            if !gateway {
                eprintln!("Error: HTTPRoute/Filters tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
        }
        "HTTPRoute/Filters/Redirect" => {
            if !gateway {
                eprintln!("Error: HTTP Redirect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
        }
        "HTTPRoute/Filters/Security" => {
            if !gateway {
                eprintln!("Error: HTTP Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
        }
        "HTTPRoute/Filters/HeaderModifier" => {
            if !gateway {
                eprintln!("Error: HTTP Header Modifier tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HeaderModifierTestSuite));
        }
        "HTTPRoute/Protocol" => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::HttpsTestSuite));
            }
        }
        "HTTPRoute/Protocol/WebSocket" => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
        }
        "HTTPRoute/Protocol/HTTPS" => {
            if !gateway {
                eprintln!("Error: HTTPS tests only support Gateway mode. Use -g flag.");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        // GRPCRoute 资源
        "GRPCRoute" => {
            // 运行 GRPCRoute 全部测试
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
            }
        }
        "GRPCRoute/Basic" => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
        }
        "GRPCRoute/Match" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/Match tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
        }
        "GRPCRoute/TLS" => {
            if !gateway {
                eprintln!("Error: GRPCRoute/TLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        // TCP/UDP 资源
        "tcp" | "TCPRoute" | "TCPRoute/Basic" => {
            runner.add_suite(Box::new(suites::TcpTestSuite));
        }
        "udp" | "UDPRoute" | "UDPRoute/Basic" => {
            runner.add_suite(Box::new(suites::UdpTestSuite));
        }
        // Gateway 资源
        "Gateway" => {
            if !gateway {
                eprintln!("Error: Gateway tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
            runner.add_suite(Box::new(suites::RealIpTestSuite));
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        "Gateway/Security" => {
            if !gateway {
                eprintln!("Error: Gateway/Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
        }
        "Gateway/RealIP" => {
            if !gateway {
                eprintln!("Error: Gateway/RealIP tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpTestSuite));
        }
        "Gateway/TLS/BackendTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/BackendTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsTestSuite));
        }
        "Gateway/TLS/GatewayTLS" => {
            if !gateway {
                eprintln!("Error: Gateway/TLS/GatewayTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GatewayTlsTestSuite));
        }
        "EdgionPlugins/DebugAccessLog" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/DebugAccessLog tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        "EdgionPlugins/PluginCondition" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginConditionTestSuite));
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/PluginCondition/AllConditions" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/PluginCondition/AllConditions tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllConditionsTestSuite));
        }
        "EdgionPlugins/JwtAuth" => {
            if !gateway {
                eprintln!("Error: EdgionPlugins/JwtAuth tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::JwtAuthTestSuite));
        }
        "Gateway/ListenerHostname" => {
            if !gateway {
                eprintln!("Error: Gateway/ListenerHostname tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::ListenerHostnameTestSuite));
        }
        "Gateway/AllowedRoutes/Same" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Same tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesSameNamespaceTestSuite));
        }
        "Gateway/AllowedRoutes/All" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/All tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesAllNamespacesTestSuite));
        }
        "Gateway/AllowedRoutes/Kinds" => {
            if !gateway {
                eprintln!("Error: Gateway/AllowedRoutes/Kinds tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::AllowedRoutesKindsTestSuite));
        }
        "Gateway/Combined" => {
            if !gateway {
                eprintln!("Error: Gateway/Combined tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CombinedScenariosTestSuite));
        }
        "Gateway/PortConflict" => {
            if !gateway {
                eprintln!("Error: Gateway/PortConflict tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PortConflictTestSuite));
        }
        "Gateway/Dynamic" => {
            if !gateway {
                eprintln!("Error: Gateway/Dynamic tests require --gateway flag");
                std::process::exit(1);
            }
            match phase {
                Some("initial") => {
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                }
                Some("update") => {
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                None => {
                    // 默认运行两个阶段
                    runner.add_suite(Box::new(suites::InitialPhaseTestSuite));
                    runner.add_suite(Box::new(suites::UpdatePhaseTestSuite));
                }
                _ => {
                    eprintln!("Error: Invalid phase '{}'. Use 'initial' or 'update'", phase.unwrap());
                    std::process::exit(1);
                }
            }
        }
        // EdgionTls 资源
        "EdgionTls" => {
            if !gateway {
                eprintln!("Error: EdgionTls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
            runner.add_suite(Box::new(suites::MtlsTestSuite));
        }
        "EdgionTls/https" => {
            if !gateway {
                eprintln!("Error: EdgionTls/https tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        "EdgionTls/grpctls" => {
            if !gateway {
                eprintln!("Error: EdgionTls/grpctls tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        "EdgionTls/mTLS" => {
            if !gateway {
                eprintln!("Error: EdgionTls/mTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsTestSuite));
        }
        "EdgionTls/cipher" => {
            if !gateway {
                eprintln!("Error: EdgionTls/cipher tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::CipherTestSuite));
        }
        // ReferenceGrant Status tests
        "ReferenceGrant/Status" | "ref-grant-status" => {
            if !gateway {
                eprintln!("Error: ReferenceGrant/Status tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RefGrantStatusTestSuite));
        }
        // 运行所有测试
        "all" => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
            runner.add_suite(Box::new(suites::TcpTestSuite));
            runner.add_suite(Box::new(suites::UdpTestSuite));
            if gateway {
                runner.add_suite(Box::new(suites::HttpMatchTestSuite));
                runner.add_suite(Box::new(suites::HttpsTestSuite));
                runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
                runner.add_suite(Box::new(suites::RealIpTestSuite));
                runner.add_suite(Box::new(suites::SecurityTestSuite));
                runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
                runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
                runner.add_suite(Box::new(suites::PluginLogsTestSuite));
                runner.add_suite(Box::new(suites::LBRoundRobinTestSuite));
                runner.add_suite(Box::new(suites::LBConsistentHashTestSuite));
                runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
                runner.add_suite(Box::new(suites::TimeoutTestSuite));
                runner.add_suite(Box::new(suites::MtlsTestSuite));
            }
        }
        _ => {
            eprintln!("Error: Unknown suite: {}", suite);
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化 rustls（仅一次）
    INIT.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    });

    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).init();
    }

    // 解析 suite 名称
    let suite = resolve_suite(
        cli.resource.as_deref(),
        cli.item.as_deref(),
        cli.legacy_command.as_deref(),
    );

    // 获取Port config key
    let port_key = suite_to_port_key(&suite);

    // Determine ports and host based on gateway flag
    let (
        http_port,
        grpc_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        websocket_port,
        https_port,
        grpc_https_port,
        http_host,
        grpc_host,
    ) = if cli.gateway {
        // Gateway mode: load ports from ports.json
        match port_config::PortConfig::load() {
            Ok(config) => {
                let ports = config.get_ports(port_key);
                // Select http_host based on suite
                let http_host = match suite.as_str() {
                    "Gateway/TLS/GatewayTLS" => "gateway-tls.test.com",
                    _ => "test.example.com",
                };
                (
                    ports.http.unwrap_or(31000),
                    ports.grpc.unwrap_or(ports.http.unwrap_or(31000)),
                    ports.tcp.unwrap_or(31090),
                    ports.tcp_filtered.unwrap_or(31091),
                    ports.udp.unwrap_or(31100),
                    ports.http.unwrap_or(31000),
                    ports.https.unwrap_or(ports.http.map(|p| p + 1).unwrap_or(31001)),
                    ports.grpc_tls.unwrap_or(31070),
                    Some(http_host.to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
            Err(e) => {
                eprintln!("Warning: Failed to load ports.json: {}. Using default ports.", e);
                (
                    31000,
                    31000,
                    31090,
                    31091,
                    31100,
                    31000,
                    31001,
                    31070,
                    Some("test.example.com".to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
        }
    } else {
        // Direct mode: use CLI provided ports
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.tcp_port + 1,
            cli.udp_port,
            cli.websocket_port,
            cli.https_port,
            cli.grpc_https_port,
            None,
            None,
        )
    };

    let mode_name = if cli.gateway { "Gateway" } else { "Direct" };

    println!("\n========================================");
    println!("Edgion 测试客户端");
    println!("========================================");
    println!("Mode: {}", mode_name);
    println!("Suite: {}", suite);
    println!("Target: {}:{}", cli.target_host, http_port);
    println!("========================================\n");

    // Get access log path from environment variable
    let access_log_path =
        std::env::var("EDGION_TEST_ACCESS_LOG_PATH").unwrap_or_else(|_| "examples/testing/logs/access.log".to_string());

    let context = TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        websocket_port,
        tcp_port,
        tcp_filtered_port,
        udp_port,
        https_port,
        grpc_https_port,
        cli.admin_port,
        http_host.clone(),
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let mut runner = TestRunner::new(context);

    // Add test suite
    add_suites_for_suite(&mut runner, &suite, cli.gateway, cli.phase.as_deref());

    let start_time = Instant::now();
    let results = runner.run().await;
    let total_duration = start_time.elapsed();

    let console_reporter = ConsoleReporter::new();
    console_reporter.report(&results, total_duration);

    if cli.json {
        let json_reporter = JsonReporter::new();
        json_reporter.save_to_file(&results, total_duration, &cli.json_output)?;
        println!("\n✓ JSON 报告已保存到: {}", cli.json_output);
    }

    if results.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}
