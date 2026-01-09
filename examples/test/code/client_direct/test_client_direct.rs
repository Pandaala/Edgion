// Edgion 直接测试客户端
// 直接测试 test_server 连通性（不通过 Gateway）
//
// 用法:
//   cargo run --example test_client_direct [OPTIONS] [COMMAND]
//
// 测试:
//   - http      HTTP 基础测试
//   - grpc      gRPC 基础测试
//   - websocket WebSocket 测试
//   - tcp       TCP 测试
//   - udp       UDP 测试
//   - all       运行所有测试

// 复用 client 的模块
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
#[command(about = "Edgion 直接测试客户端 - 测试 test_server 连通性")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// 目标主机
    #[arg(long, default_value = "127.0.0.1")]
    target_host: String,

    /// HTTP 服务器端口
    #[arg(long, default_value = "30001")]
    http_port: u16,

    /// gRPC 服务器端口
    #[arg(long, default_value = "30021")]
    grpc_port: u16,

    /// WebSocket 服务器端口
    #[arg(long, default_value = "30005")]
    websocket_port: u16,

    /// TCP 服务器端口
    #[arg(long, default_value = "30010")]
    tcp_port: u16,

    /// UDP 服务器端口
    #[arg(long, default_value = "30011")]
    udp_port: u16,

    /// 详细输出
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// HTTP 基础测试
    Http,
    /// gRPC 基础测试
    Grpc,
    /// WebSocket 测试
    Websocket,
    /// TCP 测试
    Tcp,
    /// UDP 测试
    Udp,
    /// 运行所有直接测试
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
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    println!("\n========================================");
    println!("Edgion 直接测试客户端");
    println!("========================================");
    println!("模式: Direct (不通过 Gateway)");
    println!("目标: {}:{}", cli.target_host, cli.http_port);
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
