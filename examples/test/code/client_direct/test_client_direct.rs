// Edgion Direct Test Client
// Direct test of test_server connectivity (without Gateway)
//
// Usage:
//   cargo run --example test_client_direct [OPTIONS] [COMMAND]
//
// Tests:
//   - http      HTTP basic tests
//   - grpc      gRPC basic tests
//   - websocket WebSocket tests
//   - tcp       TCP tests
//   - udp       UDP tests
//   - all       Run all tests

// Reuse client modules
#![allow(dead_code)]
#![allow(unused_imports)]
#[path = "../client/framework.rs"]
mod framework;
#[path = "../client/log_analyzer.rs"]
mod log_analyzer;
#[path = "../client/reporter.rs"]
mod reporter;
#[path = "../client/suites/mod.rs"]
mod suites;

use anyhow::Result;
use clap::{Parser, Subcommand};
use framework::{TestContext, TestRunner};
use reporter::ConsoleReporter;
use std::path::PathBuf;
use std::sync::Once;
use std::time::Instant;

static INIT: Once = Once::new();

#[derive(Parser, Debug)]
#[command(name = "test-client-direct")]
#[command(about = "Edgion Direct Test Client - Test test_server connectivity")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Target host
    #[arg(long, default_value = "127.0.0.1")]
    target_host: String,

    /// HTTP server port
    #[arg(long, default_value = "30001")]
    http_port: u16,

    /// gRPC server port
    #[arg(long, default_value = "30021")]
    grpc_port: u16,

    /// WebSocket server port
    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    /// TCP server port
    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    /// UDP server port
    #[arg(long, default_value = "30011")]
    udp_port: u16,

    /// 详细输出
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// HTTP basic tests
    Http,
    /// gRPC basic tests
    Grpc,
    /// WebSocket tests
    Websocket,
    /// TCP tests
    Tcp,
    /// UDP tests
    Udp,
    /// Run all direct tests
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

    println!("\n========================================");
    println!("Edgion Direct Test Client");
    println!("========================================");
    println!("Mode: Direct (without Gateway)");
    println!("Target: {}:{}", cli.target_host, cli.http_port);
    println!("========================================\n");

    let context = TestContext::new(
        cli.target_host.clone(),
        cli.http_port,
        cli.grpc_port,
        cli.websocket_port,
        cli.tcp_port,
        cli.tcp_port, // tcp_filtered_port (not used in direct mode)
        cli.udp_port,
        443,   // https_port (not used in direct mode)
        443,   // grpc_https_port (not used in direct mode)
        8080,  // admin_port (not used in direct mode)
        None,  // http_host
        None,  // grpc_host
        false, // gateway mode = false
        cli.verbose,
        PathBuf::from("/dev/null"), // access_log_path (not used)
    );

    let mut runner = TestRunner::new(context);

    match cli.command {
        Commands::Http => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
        }
        Commands::Grpc => {
            runner.add_suite(Box::new(suites::GrpcTestSuite));
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
        Commands::All => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
            runner.add_suite(Box::new(suites::GrpcTestSuite));
            runner.add_suite(Box::new(suites::WebSocketTestSuite));
            runner.add_suite(Box::new(suites::TcpTestSuite));
            runner.add_suite(Box::new(suites::UdpTestSuite));
        }
    }

    let start_time = Instant::now();
    let results = runner.run().await;
    let total_duration = start_time.elapsed();

    let console_reporter = ConsoleReporter::new();
    console_reporter.report(&results, total_duration);

    if results.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}
