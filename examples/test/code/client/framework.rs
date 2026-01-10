// Test framework core

use async_trait::async_trait;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// Test context - contains all configuration information needed for testing
#[derive(Clone)]
pub struct TestContext {
    pub target_host: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub websocket_port: u16,
    pub tcp_port: u16,
    pub tcp_filtered_port: u16, // For testing sectionName matching
    pub udp_port: u16,
    pub https_port: u16,
    pub grpc_https_port: u16,
    pub http_client: reqwest::Client,
    pub http_host: Option<String>,
    pub grpc_host: Option<String>,
    #[allow(dead_code)]
    pub gateway: bool,
    #[allow(dead_code)]
    pub verbose: bool,
    pub access_log_path: PathBuf,
}

impl TestContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target_host: String,
        http_port: u16,
        grpc_port: u16,
        websocket_port: u16,
        tcp_port: u16,
        tcp_filtered_port: u16,
        udp_port: u16,
        https_port: u16,
        grpc_https_port: u16,
        http_host: Option<String>,
        grpc_host: Option<String>,
        gateway: bool,
        verbose: bool,
        access_log_path: PathBuf,
    ) -> Self {
        // Configure HTTP client to accept self-signed certificates for HTTPS testing
        let mut client_builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(true);
        
        // Add DNS resolution for HTTPS hosts to support proper SNI
        // This maps hostnames to 127.0.0.1 so the client sends correct SNI
        if let Some(ref host) = http_host {
            // Resolve HTTP host for HTTPS connections
            let addr: std::net::SocketAddr = format!("127.0.0.1:{}", https_port).parse().unwrap();
            client_builder = client_builder.resolve(host, addr);
            // Also resolve for regular HTTP port
            let http_addr: std::net::SocketAddr = format!("127.0.0.1:{}", http_port).parse().unwrap();
            client_builder = client_builder.resolve(host, http_addr);
        }
        if let Some(ref host) = grpc_host {
            // Resolve gRPC host for gRPC-TLS connections
            let addr: std::net::SocketAddr = format!("127.0.0.1:{}", grpc_https_port).parse().unwrap();
            client_builder = client_builder.resolve(host, addr);
        }
        
        let http_client = client_builder
            .build()
            .expect("Failed to create HTTP client");

        Self {
            target_host,
            http_port,
            grpc_port,
            websocket_port,
            tcp_port,
            tcp_filtered_port,
            udp_port,
            https_port,
            grpc_https_port,
            http_client,
            http_host,
            grpc_host,
            gateway,
            verbose,
            access_log_path,
        }
    }

    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.target_host, self.http_port)
    }

    #[allow(dead_code)]
    pub fn grpc_url(&self) -> String {
        format!("http://{}:{}", self.target_host, self.grpc_port)
    }

    pub fn websocket_url(&self) -> String {
        format!("ws://{}:{}/ws", self.target_host, self.websocket_port)
    }

    pub fn tcp_addr(&self) -> String {
        format!("{}:{}", self.target_host, self.tcp_port)
    }

    pub fn tcp_filtered_addr(&self) -> String {
        format!("{}:{}", self.target_host, self.tcp_filtered_port)
    }

    pub fn udp_addr(&self) -> String {
        format!("{}:{}", self.target_host, self.udp_port)
    }

    #[allow(dead_code)]
    pub fn https_url(&self) -> String {
        format!("https://{}:{}", self.target_host, self.https_port)
    }

    #[allow(dead_code)]
    pub fn grpc_https_url(&self) -> String {
        format!("https://{}:{}", self.target_host, self.grpc_https_port)
    }
}

/// Test result
#[derive(Debug, Clone)]
pub struct TestResult {
    pub passed: bool,
    pub duration: Duration,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl TestResult {
    pub fn passed(duration: Duration) -> Self {
        Self {
            passed: true,
            duration,
            message: None,
            error: None,
        }
    }

    pub fn passed_with_message(duration: Duration, message: String) -> Self {
        Self {
            passed: true,
            duration,
            message: Some(message),
            error: None,
        }
    }

    pub fn failed(duration: Duration, error: String) -> Self {
        Self {
            passed: false,
            duration,
            message: None,
            error: Some(error),
        }
    }
}

/// Test function type
pub type TestFn = fn(TestContext) -> Pin<Box<dyn Future<Output = TestResult> + Send>>;

/// Test case
pub struct TestCase {
    pub name: String,
    #[allow(dead_code)]
    pub description: String,
    pub test_fn: TestFn,
}

impl TestCase {
    pub fn new(name: impl Into<String>, description: impl Into<String>, test_fn: TestFn) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            test_fn,
        }
    }

    pub async fn run(&self, ctx: &TestContext) -> TestResult {
        let start = Instant::now();

        // Timeout control
        match tokio::time::timeout(Duration::from_secs(30), (self.test_fn)(ctx.clone())).await {
            Ok(result) => result,
            Err(_) => TestResult::failed(start.elapsed(), "Test timed out after 30 seconds".to_string()),
        }
    }
}

/// Suite result
pub struct SuiteResult {
    pub name: String,
    pub test_results: Vec<(String, TestResult)>,
    pub duration: Duration,
}

impl SuiteResult {
    pub fn passed_count(&self) -> usize {
        self.test_results.iter().filter(|(_, r)| r.passed).count()
    }

    pub fn failed_count(&self) -> usize {
        self.test_results.iter().filter(|(_, r)| !r.passed).count()
    }

    pub fn total_count(&self) -> usize {
        self.test_results.len()
    }
}

/// Test suite trait
#[async_trait]
pub trait TestSuite: Send + Sync {
    fn name(&self) -> &str;
    fn test_cases(&self) -> Vec<TestCase>;

    async fn setup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn teardown(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(&self, ctx: &TestContext) -> SuiteResult {
        let start = Instant::now();
        let test_cases = self.test_cases();
        let mut test_results = Vec::new();

        // Setup
        if let Err(e) = self.setup().await {
            eprintln!("Setup failed for {}: {}", self.name(), e);
            return SuiteResult {
                name: self.name().to_string(),
                test_results,
                duration: start.elapsed(),
            };
        }

        // Run tests
        for test in test_cases {
            let result = test.run(ctx).await;
            test_results.push((test.name.clone(), result));
        }

        // Teardown
        if let Err(e) = self.teardown().await {
            eprintln!("Teardown failed for {}: {}", self.name(), e);
        }

        SuiteResult {
            name: self.name().to_string(),
            test_results,
            duration: start.elapsed(),
        }
    }
}

/// Test runner
pub struct TestRunner {
    context: TestContext,
    suites: Vec<Box<dyn TestSuite>>,
}

impl TestRunner {
    pub fn new(context: TestContext) -> Self {
        Self {
            context,
            suites: Vec::new(),
        }
    }

    pub fn add_suite(&mut self, suite: Box<dyn TestSuite>) {
        self.suites.push(suite);
    }

    pub async fn run(&self) -> TestResults {
        let mut suite_results = Vec::new();

        for suite in &self.suites {
            let result = suite.run(&self.context).await;
            suite_results.push(result);
        }

        TestResults { suite_results }
    }
}

/// 所有Test result
pub struct TestResults {
    pub suite_results: Vec<SuiteResult>,
}

impl TestResults {
    pub fn total_passed(&self) -> usize {
        self.suite_results.iter().map(|s| s.passed_count()).sum()
    }

    pub fn total_failed(&self) -> usize {
        self.suite_results.iter().map(|s| s.failed_count()).sum()
    }

    pub fn total_tests(&self) -> usize {
        self.suite_results.iter().map(|s| s.total_count()).sum()
    }

    pub fn pass_rate(&self) -> f64 {
        let total = self.total_tests();
        if total == 0 {
            return 100.0;
        }
        (self.total_passed() as f64 / total as f64) * 100.0
    }

    pub fn has_failures(&self) -> bool {
        self.total_failed() > 0
    }
}
