use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "edgion")]
#[command(version, about = "Edgion - High-performance API Gateway", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the operator (configuration server)
    Operator(OperatorCommand),

    /// Run the gateway in client mode connecting to an external operator
    Gateway(GatewayCommand),

    /// Run the gateway with an embedded operator and configuration loader
    GatewayWithOperator(GatewayWithOperatorCommand),
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum LoaderKind {
    Filesystem,
    Etcd,
}

#[derive(Args, Debug)]
pub struct OperatorCommand {
    /// Optional gRPC listen address for the operator
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

#[derive(Args, Debug)]
pub struct GatewayCommand {
    /// Gateway class name (required for all gateway modes)
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// gRPC listen address for the gateway control plane
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Connect to an external operator over gRPC
    #[arg(long, value_name = "ADDR")]
    pub server_addr: String,
}

#[derive(Args, Debug)]
pub struct GatewayWithOperatorCommand {
    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// gRPC listen address for the gateway control plane
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

#[derive(Args, Debug)]
pub struct LoaderArgs {
    /// Configuration loader type
    #[arg(long, value_enum, value_name = "TYPE")]
    pub loader: LoaderKind,

    /// Root directory when using the filesystem loader
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,

    /// Etcd endpoints (repeat the flag to provide multiple values)
    #[arg(long = "etcd-endpoint", value_name = "URL")]
    pub etcd_endpoint: Vec<String>,

    /// Etcd key prefix
    #[arg(long = "etcd-prefix", value_name = "PREFIX")]
    pub etcd_prefix: Option<String>,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub fn validate_and_run(&self) {
        match &self.command {
            Command::Operator(cmd) => {
                println!("Starting operator mode");
                if let Some(addr) = &cmd.grpc_listen {
                    println!("  gRPC listen: {}", addr);
                }
                validate_loader_args("operator", &cmd.loader);
            }
            Command::Gateway(cmd) => {
                println!("Starting gateway mode (external operator)");
                println!("  Gateway class: {}", cmd.gateway_class);
                println!("  Operator gRPC server: {}", cmd.server_addr);
                if let Some(addr) = &cmd.grpc_listen {
                    println!("  Gateway gRPC listen: {}", addr);
                }
            }
            Command::GatewayWithOperator(cmd) => {
                println!("Starting gateway mode with embedded operator");
                println!("  Gateway class: {}", cmd.gateway_class);
                if let Some(addr) = &cmd.grpc_listen {
                    println!("  Gateway gRPC listen: {}", addr);
                }
                validate_loader_args("gateway", &cmd.loader);
            }
        }
    }
}

fn validate_loader_args(context: &str, loader: &LoaderArgs) {
    match loader.loader {
        LoaderKind::Filesystem => {
            if let Some(dir) = &loader.dir {
                println!("  Loader: filesystem");
                println!("  Config directory: {}", dir);
            } else {
                exit_with_error(&format!(
                    "--dir must be provided for filesystem loader in {} mode",
                    context
                ));
            }

            if !loader.etcd_endpoint.is_empty() || loader.etcd_prefix.is_some() {
                println!("  note: etcd flags are ignored when using the filesystem loader");
            }
        }
        LoaderKind::Etcd => {
            if loader.etcd_endpoint.is_empty() {
                exit_with_error(&format!(
                    "At least one --etcd-endpoint must be provided for etcd loader in {} mode",
                    context
                ));
            }
            println!("  Loader: etcd");
            for endpoint in &loader.etcd_endpoint {
                println!("  Etcd endpoint: {}", endpoint);
            }
            let prefix = loader
                .etcd_prefix
                .clone()
                .unwrap_or_else(|| "/".to_string());
            println!("  Etcd prefix: {}", prefix);
            if loader.dir.is_some() {
                println!("  note: --dir is ignored when using the etcd loader");
            }
        }
    }
}

fn exit_with_error(message: &str) -> ! {
    eprintln!("Error: {}", message);
    std::process::exit(1);
}
