//! Unified Secret reference for Edgion CRDs (LinkSys, EdgionAcme, ...).
//!
//! - `name` 必填
//! - `namespace` 可选；缺省视为 owner 资源自身的 namespace
//! - 跨 ns（`namespace != owner.namespace`）必须有 Gateway API `ReferenceGrant` 授权
//! - 不承载 key 自定义字段：Secret 键走 well-known 固定约定（见 spec §5）
//!
//! # Zeroize scope (transport)
//!
//! When the controller resolves a `SecretReference`, it embeds the full K8s
//! `Secret` object into the owning resource (e.g. `*Auth.secret` /
//! [`super::tls::TlsCerts::secret`]) and serializes it via `serde_json` over
//! gRPC through conf_sync to the Gateway. Those serialization, gRPC and
//! runtime-store buffers are plain `Vec<u8>` / `String` and are **not**
//! zero-wiped. Only private-key-bearing materials parsed out of the Secret
//! on the Gateway side (`TlsMaterial.client_key_pem`,
//! `AcmeAccountMaterial.credentials`) enjoy `Zeroizing` protection. Treat
//! the embedded-Secret path as defense-in-depth only.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretReference {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl SecretReference {
    /// 解析出目标 namespace（缺省回落到 owner ns）
    pub fn effective_namespace<'a>(&'a self, owner_ns: &'a str) -> &'a str {
        self.namespace.as_deref().unwrap_or(owner_ns)
    }

    /// 是否跨 namespace 引用
    pub fn is_cross_namespace(&self, owner_ns: &str) -> bool {
        match self.namespace.as_deref() {
            Some(ns) => ns != owner_ns,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SecretReference;

    #[test]
    fn deserialize_minimal() {
        let yaml = "name: my-secret";
        let sr: SecretReference = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sr.name, "my-secret");
        assert_eq!(sr.namespace, None);
    }

    #[test]
    fn deserialize_with_namespace() {
        let yaml = "name: my-secret\nnamespace: credentials";
        let sr: SecretReference = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sr.name, "my-secret");
        assert_eq!(sr.namespace.as_deref(), Some("credentials"));
    }

    #[test]
    fn name_is_required() {
        let yaml = "namespace: credentials";
        let result: Result<SecretReference, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        // 旧内联字段（usernameKey / passwordKey）彻底不接受
        let yaml = "name: my-secret\nusernameKey: user\npasswordKey: pass";
        let result: Result<SecretReference, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "usernameKey/passwordKey 必须被拒绝");
    }
}
