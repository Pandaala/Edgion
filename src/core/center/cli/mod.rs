use anyhow::Result;
use clap::Parser;
use crate::core::center::config::CenterConfig;

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    /// Configuration file path (TOML)
    #[arg(short = 'c', long, default_value = "config/edgion-center.toml")]
    pub config_file: String,
}

impl EdgionCenterCli {
    pub fn parse_args() -> Self { Self::parse() }

    pub async fn run(&self) -> Result<()> {
        let content = std::fs::read_to_string(&self.config_file)
            .unwrap_or_default();
        let _config: CenterConfig = toml::from_str(&content).unwrap_or_default();
        tracing::info!(component = "center", "edgion-center starting (stub)");
        // Full startup wired in later tasks
        Ok(())
    }
}
