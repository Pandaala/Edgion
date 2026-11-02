//! Shared Kubernetes client construction for Controller and CLI entry points.

use kube::{Client, Config};
use std::time::Duration;

/// Infer authentication and cluster settings using the standard Kube loaders.
pub async fn default_client() -> anyhow::Result<Client> {
    let config = client_config(Config::infer().await?);
    Ok(Client::try_from(config)?)
}

/// Keep Edgion's request policy explicit across Kube major upgrades.
fn client_config(mut config: Config) -> Config {
    config.default_retry = false;
    config.read_timeout = Some(Duration::from_secs(295));
    // In-cluster inference has already loaded root_cert. Retain the static
    // snapshot used by Edgion rather than introducing periodic CA-file reloads.
    // With no explicit CA, Kube 4.2 uses the platform certificate verifier.
    config.root_cert_file = None;
    config
}

#[cfg(test)]
#[path = "../tests/unit/kube_client_test.rs"]
mod tests;
