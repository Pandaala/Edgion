// Edgion 统一测试客户端
// 支持所有协议的测试: HTTP/HTTPS, gRPC, WebSocket, TCP, UDP

#[path = "./test_client/framework.rs"]
mod framework;
#[path = "./test_client/reporter.rs"]
mod reporter;
#[path = "./test_client/suites/mod.rs"]
mod suites;

use anyhow::Result;
use clap::{Parser, Subcommand};
use framework::{TestContext, TestRunner};
use reporter::{ConsoleReporter, JsonReporter};
use std::time::Instant;

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
    
    #[arg(long, default_value = "18443")]
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
    Http,
    Https,
    Grpc,
    GrpcHttps,
    Websocket,
    Tcp,
    Udp,
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }
    
    // Determine ports and host based on gateway flag
    let (http_port, grpc_port, tcp_port, udp_port, websocket_port, https_port, grpc_https_port, http_host, grpc_host) = if cli.gateway {
        // Gateway mode: use Gateway ports
        (
            10080,  // Gateway HTTP port
            10080,  // Gateway HTTP port (gRPC uses HTTP listener)
            19000,  // Gateway TCP port
            19002,  // Gateway UDP port
            10080,  // WebSocket through HTTP Gateway
            18443,  // Gateway HTTPS port
            18443,  // Gateway gRPC-HTTPS port
            Some("test.example.com".to_string()),
            Some("grpc.example.com".to_string()),
        )
    } else {
        // Direct mode: use CLI provided ports
        (
            cli.http_port,
            cli.grpc_port,
            cli.tcp_port,
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
    
    let context = TestContext::new(
        cli.target_host.clone(),
        http_port,
        grpc_port,
        websocket_port,
        tcp_port,
        udp_port,
        https_port,
        grpc_https_port,
        http_host.clone(),
        grpc_host,
        cli.gateway,
        cli.verbose,
    );
    
    let mut runner = TestRunner::new(context);
    
    match cli.command {
        Commands::Http => {
            runner.add_suite(Box::new(suites::HttpTestSuite));
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
        Commands::GrpcHttps => {
            if !cli.gateway {
                eprintln!("Error: gRPC-HTTPS tests only support Gateway mode. Use -g flag.");
                std::process::exit(1);
            }
            runner.add_suite(Box::new(suites::GrpcHttpsTestSuite));
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
            if cli.gateway {
                runner.add_suite(Box::new(suites::HttpsTestSuite));
                runner.add_suite(Box::new(suites::GrpcHttpsTestSuite));
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
