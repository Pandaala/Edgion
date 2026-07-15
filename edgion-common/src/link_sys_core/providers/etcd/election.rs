//! Thin wrapper over etcd-client 0.18's v3election service (spec §3.2).
//!
//! `campaign` blocks server-side until this candidate wins; it must run in a
//! cancellable task and MUST NOT share a Client that has a channel-wide request
//! timeout, or the timeout kills the blocking call. `election_session()` builds
//! such a Client.

use anyhow::Result;
use etcd_client::Client;
pub use etcd_client::LeaderKey;

// Canonical gRPC "Unimplemented" code. Compared numerically because etcd-client
// 0.18 exposes `tonic::Status` from tonic 0.14, while the workspace `tonic` is
// 0.12 — the two `tonic::Code` types are distinct, so a named-variant comparison
// would not type-check. `tonic::Code::Unimplemented as i32` is the stable value.
const GRPC_CODE_UNIMPLEMENTED: i32 = 12;

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
    /// call could not complete. The sole caller (`refresh_current_leader`) is
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
            Err(etcd_client::Error::GRpcStatus(s)) if s.code() as i32 == GRPC_CODE_UNIMPLEMENTED => {
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
mod tests {
    use crate::link_sys_core::credential::CredentialState;
    use crate::link_sys_core::providers::etcd::EtcdLinkClient;
    use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

    /// Build + connect an `EtcdLinkClient` against a local etcd. Inlined (rather
    /// than sharing ops.rs's private test helper) to keep the test modules
    /// independent — see Task 4 deviation note.
    async fn test_client() -> EtcdLinkClient {
        let config = EtcdClientConfig {
            endpoints: vec!["http://localhost:2379".to_string()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
        };
        let client = EtcdLinkClient::from_config("default/etcd-test", &config, &CredentialState::NotRequired)
            .expect("from_config must succeed");
        client.init().await.expect("etcd init must connect");
        client
    }

    #[tokio::test]
    #[ignore = "requires a real etcd serving v3election on localhost:2379"]
    async fn campaign_then_leader_then_resign() {
        let client = test_client().await;
        let mut sess = client.election_session().await.unwrap();
        let lease = client.lease_grant(30).await.unwrap();
        let lk = sess
            .campaign("edgion/leader-test", b"ctrl-a".to_vec(), lease)
            .await
            .unwrap();
        let leader = sess.leader("edgion/leader-test").await.unwrap();
        assert_eq!(leader.as_deref(), Some(&b"ctrl-a"[..]));
        sess.resign(lk).await.unwrap();
        let leader = sess.leader("edgion/leader-test").await.unwrap();
        assert!(leader.is_none(), "after resign there is no leader");
    }
}
