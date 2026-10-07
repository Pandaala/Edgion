//! Actual-owner test utility; absent from normal production builds.
use super::RedisLinkClient;
use crate::link_sys::credential::CredentialState;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::redis::RedisClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, LinkSysSpec, SystemConfig};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::BTreeMap;
use std::sync::Arc;

impl RedisLinkClient {
    pub fn from_test_config(
        label: &str,
        config: &RedisClientConfig,
        auth: &CredentialState<BasicAuthMaterial>,
    ) -> anyhow::Result<Self> {
        let (namespace, name) = label.split_once('/').unwrap_or(("integration", label));
        let mut config = config.clone();
        if let Some(slot) = config.auth.as_mut().map(|auth| &mut auth.slot) {
            let material = crate::link_sys::credential::require_basic_material(auth, "Redis test fixture")?
                .ok_or_else(|| anyhow::anyhow!("Redis test fixture requires declared auth material"))?;
            let mut data = BTreeMap::from([("password".into(), ByteString(material.password.as_bytes().to_vec()))]);
            if let Some(username) = &material.username {
                data.insert("username".into(), ByteString(username.as_bytes().to_vec()));
            }
            slot.secret = Some(Secret {
                metadata: kube::core::ObjectMeta {
                    name: Some(slot.secret_ref.name.clone()),
                    namespace: Some(slot.secret_ref.namespace.as_deref().unwrap_or(namespace).into()),
                    ..Default::default()
                },
                data: Some(data),
                ..Default::default()
            });
        }
        let mut owner = LinkSys::new(
            name,
            LinkSysSpec {
                config: SystemConfig::Redis(config),
                current_status: None,
                unknown_fields: Vec::new(),
            },
        );
        owner.metadata.namespace = Some(namespace.into());
        Self::from_link_sys(Arc::new(owner))
    }
}
