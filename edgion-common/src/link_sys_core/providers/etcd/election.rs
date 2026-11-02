//! Thin wrapper over etcd-client 0.21's v3election service (spec §3.2).
//!
//! `campaign` blocks server-side until this candidate wins; it must run in a
//! cancellable task and MUST NOT share a Client that has a channel-wide request
//! timeout, or the timeout kills the blocking call. `election_session()` builds
//! such a Client.

use anyhow::Result;
use etcd_client::Client;
pub use etcd_client::LeaderKey;

pub struct ElectionSession {
    client: Client,
}

impl ElectionSession {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Blocks server-side until elected. Cancel by dropping the future.
    pub async fn campaign(&mut self, name: &str, value: Vec<u8>, lease_id: i64) -> Result<LeaderKey> {
        let resp = self
            .client
            .campaign(name, value, lease_id)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd CAMPAIGN {}: {:?}", name, e))?;
        resp.leader()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Etcd CAMPAIGN {} returned no LeaderKey", name))
    }

    /// Current leader's value (for the not-leader 409 body). `None` = no leader
    /// is known — either the election is genuinely vacant (etcd returns a
    /// non-Unimplemented error, e.g. `Unknown "election: no leader"`) or the
    /// call could not complete. The Controller candidate refresher is
    /// best-effort and treats an error and a vacancy identically — it just
    /// leaves the cached leader unchanged — so both collapse to `None` here and
    /// we avoid depending on a specific error code or message string.
    pub async fn leader(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.client.leader(name).await {
            Ok(resp) => Ok(resp.kv().map(|kv| kv.value().to_vec())),
            Err(_) => Ok(None),
        }
    }

    pub async fn resign(&mut self, leader_key: LeaderKey) -> Result<()> {
        self.client
            .resign(Some(etcd_client::ResignOptions::new().with_leader(leader_key)))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd RESIGN: {:?}", e))?;
        Ok(())
    }

    /// Startup probe (spec §3.2): fail fast with an actionable message if the
    /// etcd deployment does not serve v3election. Only `Unimplemented` is fatal;
    /// any other outcome (a leader, a vacant election, or a transient error)
    /// proves the service is present, so we do not depend on a "no leader"
    /// error code or message.
    pub async fn probe(&mut self) -> Result<()> {
        match self.client.leader("edgion/__v3election_probe__").await {
            Err(etcd_client::Error::GRpcStatus(s)) if s.code() == tonic::Code::Unimplemented => {
                anyhow::bail!(
                    "etcd does not serve the v3election service (Unimplemented). Etcd leader \
                     election requires v3election; some proxied/managed etcd deployments disable \
                     it. See spec §3.2."
                )
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/election_test.rs"]
mod tests;
