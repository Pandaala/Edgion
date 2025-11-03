use clap::{Parser, Subcommand, ValueEnum};

/// Edgion - High-performance API Gateway
#[derive(Parser, Debug)]
#[command(name = "edgion")]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Configuration source type
#[derive(Debug, Clone, ValueEnum)]
pub enum ConfigSource {
    /// Local directory configuration
    LocalDir,
    /// Etcd configuration
    Etcd,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run in local mode with file-based configuration
    Local {
        /// Configuration source (local-dir or etcd)
        #[arg(long, value_name = "SOURCE")]
        config_source: ConfigSource,

        /// Configuration directory path (required when config-source is local-dir)
        #[arg(long, value_name = "DIR")]
        config_dir: Option<String>,

        /// Etcd address (required when config-source is etcd)
        #[arg(long, value_name = "ADDR")]
        etcd_addr: Option<String>,

        /// Gateway class name
        #[arg(long, value_name = "CLASS")]
        gateway_class: Option<String>,
    },

    /// Run in Kubernetes mode
    Kube {
        /// Disable operator mode
        #[arg(long)]
        without_operator: bool,

        /// Gateway class name
        #[arg(long, value_name = "CLASS")]
        gateway_class: Option<String>,

        /// Operator address (required when operator mode is enabled)
        #[arg(long, value_name = "ADDR")]
        operator_addr: Option<String>,
    },
}

impl Cli {
    /// Parse command line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Validate and execute the command
    pub fn validate_and_run(&self) {
        match &self.command {
            Commands::Local {
                config_source,
                config_dir,
                etcd_addr,
                gateway_class,
            } => {
                println!("Running in LOCAL mode");

                match config_source {
                    ConfigSource::LocalDir => {
                        println!("  Config source: local-dir");
                        if let Some(dir) = config_dir {
                            println!("  Config directory: {}", dir);
                        } else {
                            eprintln!(
                                "Error: --config-dir is required when config-source is local-dir"
                            );
                            std::process::exit(1);
                        }
                    }
                    ConfigSource::Etcd => {
                        println!("  Config source: etcd");
                        if let Some(addr) = etcd_addr {
                            println!("  Etcd address: {}", addr);
                        } else {
                            eprintln!("Error: --etcd-addr is required when config-source is etcd");
                            std::process::exit(1);
                        }
                    }
                }

                if let Some(class) = gateway_class {
                    println!("  Gateway class: {}", class);
                }
            }

            Commands::Kube {
                without_operator,
                gateway_class,
                operator_addr,
            } => {
                println!("Running in KUBERNETES mode");

                if *without_operator {
                    println!("  Operator: disabled");
                } else {
                    println!("  Operator: enabled");
                    if let Some(addr) = operator_addr {
                        println!("  Operator address: {}", addr);
                    } else {
                        eprintln!(
                            "Error: --operator-addr is required when operator mode is enabled"
                        );
                        std::process::exit(1);
                    }
                }

                if let Some(class) = gateway_class {
                    println!("  Gateway class: {}", class);
                }
            }
        }
    }
}
