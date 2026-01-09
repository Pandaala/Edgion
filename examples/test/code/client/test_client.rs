// Edgion 统一测试客户端
// 支持所有协议的测试: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

mod framework;
mod log_analyzer;
mod port_config;
mod reporter;
mod suites;

use anyhow::Result;
use clap::{Parser, Subcommand};
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
    #[command(subcommand)]
    command: Commands,

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

    #[arg(long)]
    json: bool,

    #[arg(long, default_value = "test_report.json")]
    json_output: String,

    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    // proto test
    Http,
    HttpMatch,    // HTTP Match Rules test
    HttpRedirect, // HTTP to HTTPS redirect test
    HttpSecurity, // HTTP Security tests (hostname validation)
    Https,
    Grpc,
    GrpcMatch, // gRPC Match Rules test
    GrpcTls,
    Websocket,
    Tcp,
    Udp,

    // function test
    RealIp,
    Security,
    Mtls,            // mTLS tests
    BackendTls,      // Backend TLS tests (BackendTLSPolicy)
    PluginLogs,      // Plugin logs tests
    LbPolicy,        // LB Policy tests with log analyzer
    Timeout,         // Timeout tests
    WeightedBackend, // Weighted backend tests
    All,
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

    // Get command name for port lookup
    let command_name = match &cli.command {
        Commands::Http => "http",
        Commands::Https => "https",
        Commands::HttpMatch => "http-match",
        Commands::HttpSecurity => "http-security",
        Commands::HttpRedirect => "http-redirect",
        Commands::Grpc => "grpc",
        Commands::GrpcMatch => "grpc-match",
        Commands::GrpcTls => "grpc-tls",
        Commands::Websocket => "websocket",
        Commands::Tcp => "tcp",
        Commands::Udp => "udp",
        Commands::Mtls => "mtls",
        Commands::LbPolicy => "lb-policy",
        Commands::WeightedBackend => "weighted-backend",
        Commands::Timeout => "timeout",
        Commands::Security => "security",
        Commands::RealIp => "real-ip",
        Commands::BackendTls => "backend-tls",
        Commands::PluginLogs => "plugin-logs",
        Commands::All => "http", // Default to http for "all"
    };

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
        let suite_name = port_config::command_to_suite(command_name);
        match port_config::PortConfig::load() {
            Ok(config) => {
                let ports = config.get_ports(suite_name);
                (
                    ports.http.unwrap_or(31000),
                    ports.grpc.unwrap_or(ports.http.unwrap_or(31000)), // gRPC uses HTTP listener if not specified
                    ports.tcp.unwrap_or(31090),
                    ports.tcp_filtered.unwrap_or(31091),
                    ports.udp.unwrap_or(31100),
                    ports.http.unwrap_or(31000), // WebSocket uses HTTP port
                    ports.https.unwrap_or(ports.http.map(|p| p + 1).unwrap_or(31001)),
                    ports.grpc_tls.unwrap_or(31070),
                    Some("test.example.com".to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
            Err(e) => {
                eprintln!("Warning: Failed to load ports.json: {}. Using default ports.", e);
                // Fallback to old default ports
                (
                    31000, 31000, 31090, 31091, 31100, 31000, 31001, 31070,
                    Some("test.example.com".to_string()),
                    Some("grpc.example.com".to_string()),
                )
            }
        }
    } else {
        // Direct mode: use CLI provided ports (pointing to test_server)
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
            cli.tcp_port + 1, // tcp_filtered_port
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
    println!("模式: {}", mode_name);
    println!("目标: {}:{}", cli.target_host, http_port);
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
        http_host.clone(),
        grpc_host,
        cli.gateway,
        cli.verbose,
        PathBuf::from(access_log_path),
    );

    let mut runner = TestRunner::new(context);

    match cli.command {
        Commands::Http => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
        }
        Commands::HttpMatch => {
            if !cli.gateway {
                eprintln!("Error: HTTP Match Rules tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpMatchTestSuite));
        }
        Commands::HttpRedirect => {
            if !cli.gateway {
                eprintln!("Error: HTTP Redirect tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpRedirectTestSuite));
        }
        Commands::HttpSecurity => {
            if !cli.gateway {
                eprintln!("Error: HTTP Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
        }
        Commands::Https => {
            if !cli.gateway {
                eprintln!("Error: HTTPS tests only support Gateway mode. Use -g flag.");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::HttpsTestSuite));
        }
        Commands::Grpc => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
        }
        Commands::GrpcMatch => {
            if !cli.gateway {
                eprintln!("Error: gRPC Match Rules tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcMatchTestSuite));
        }
        Commands::GrpcTls => {
            if !cli.gateway {
                eprintln!("Error: gRPC TLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
        }
        Commands::Websocket => {
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
        }
        Commands::Tcp => {
            runner.add_suite(Box::new(suites::TcpTestSuite));
        }
        Commands::Udp => {
            runner.add_suite(Box::new(suites::UdpTestSuite));
        }
        Commands::RealIp => {
            if !cli.gateway {
                eprintln!("Error: Real IP tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::RealIpTestSuite));
        }
        Commands::Security => {
            if !cli.gateway {
                eprintln!("Error: Security tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::SecurityTestSuite));
        }
        Commands::Mtls => {
            if !cli.gateway {
                eprintln!("Error: mTLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::MtlsTestSuite));
        }
        Commands::BackendTls => {
            if !cli.gateway {
                eprintln!("Error: Backend TLS tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::BackendTlsTestSuite));
        }
        Commands::PluginLogs => {
            if !cli.gateway {
                eprintln!("Error: Plugin logs tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::PluginLogsTestSuite));
        }
        Commands::LbPolicy => {
            if !cli.gateway {
                eprintln!("Error: LB Policy tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::LBPolicyTestSuite));
        }
        Commands::Timeout => {
            if !cli.gateway {
                eprintln!("Error: Timeout tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::TimeoutTestSuite));
        }
        Commands::WeightedBackend => {
            if !cli.gateway {
                eprintln!("Error: Weighted backend tests require --gateway flag");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::WeightedBackendTestSuite));
        }
        Commands::All => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
            runner.add_suite(Box::new(suites::TcpTestSuite));
            runner.add_suite(Box::new(suites::UdpTestSuite));
            if cli.gateway {
                runner.add_suite(Box::new(suites::HttpsTestSuite));
                runner.add_suite(Box::new(suites::GrpcTlsTestSuite));
                runner.add_suite(Box::new(suites::RealIpTestSuite));
                runner.add_suite(Box::new(suites::SecurityTestSuite));
                runner.add_suite(Box::new(suites::HttpSecurityTestSuite));
                runner.add_suite(Box::new(suites::PluginLogsTestSuite));
                runner.add_suite(Box::new(suites::LBPolicyTestSuite));
                runner.add_suite(Box::new(suites::BackendTlsTestSuite));
            }
        }
    }

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
