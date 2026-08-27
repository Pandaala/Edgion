//! Secret-backed upstream credential injection.
//!
//! The request-stage plugin removes any client-supplied value immediately, but
//! defers the resolved credential until the final attempt-local upstream header
//! is built. This keeps credential material out of mirrors and downstream
//! request state while preserving ordered plugin override semantics.

use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};

use crate::plugins::http::common::{connection_listed_hop_by_hop, is_hop_by_hop};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    CredentialInjectionTarget, CredentialInjectorConfig, CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN,
};

pub struct CredentialInjector {
    name: &'static str,
    target: HeaderName,
    value: Arc<HeaderValue>,
}

impl CredentialInjector {
    pub fn create(config: &CredentialInjectorConfig) -> Option<Box<dyn RequestFilter>> {
        let resolved = config.resolved_credential.as_ref()?;
        let (target, prefix) = match &config.target {
            CredentialInjectionTarget::Bearer => (HeaderName::from_static("authorization"), "Bearer "),
            CredentialInjectionTarget::Header { name, value_prefix } => {
                (HeaderName::from_bytes(name.as_bytes()).ok()?, value_prefix.as_str())
            }
        };
        if is_hop_by_hop(target.as_str()) {
            return None;
        }

        let final_len = config.target.final_value_len(resolved.value.len())?;
        if final_len > CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN {
            return None;
        }
        let mut raw_value = String::with_capacity(final_len);
        raw_value.push_str(prefix);
        raw_value.push_str(&resolved.value);
        let mut value = HeaderValue::from_str(&raw_value).ok()?;
        value.set_sensitive(true);
        let value = Arc::new(value);

        Some(Box::new(Self {
            name: "CredentialInjector",
            target,
            value,
        }))
    }

    fn reject(log: &mut PluginLog, reason: &'static str) -> PluginRunningResult {
        log.push_err(reason);
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        }
    }
}

#[async_trait]
impl RequestFilter for CredentialInjector {
    fn name(&self) -> &str {
        self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if session.is_remote_ip_header(self.target.as_str()) {
            return Self::reject(log, "credential target is reserved");
        }

        let Some(connection_values) = session.request_header_values("connection") else {
            return Self::reject(log, "invalid Connection header");
        };
        if connection_values
            .iter()
            .flat_map(|value| connection_listed_hop_by_hop(value))
            .any(|name| name.eq_ignore_ascii_case(self.target.as_str()))
        {
            return Self::reject(log, "credential target is connection-scoped");
        }

        if session.remove_request_header(self.target.as_str()).is_err() {
            return Self::reject(log, "credential target removal failed");
        }
        if session
            .defer_credential_header(self.target.clone(), Arc::clone(&self.value))
            .is_err()
        {
            return Self::reject(log, "credential deferral failed");
        }

        log.push("credential deferred");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::SecretObjectReference;
    use edgion_resources::resources::edgion_plugins::ResolvedCredentialInjector;

    fn config(target: CredentialInjectionTarget, credential: Option<&str>) -> CredentialInjectorConfig {
        CredentialInjectorConfig {
            credential_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "provider-token".to_string(),
                namespace: None,
            },
            target,
            resolved_credential: credential.map(|value| ResolvedCredentialInjector {
                value: value.to_string(),
            }),
        }
    }

    #[test]
    fn construction_requires_valid_resolved_material() {
        assert!(CredentialInjector::create(&config(CredentialInjectionTarget::Bearer, None)).is_none());

        let oversized = "x".repeat(CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN);
        assert!(CredentialInjector::create(&config(CredentialInjectionTarget::Bearer, Some(&oversized))).is_none());

        let exact = "x".repeat(CREDENTIAL_INJECTOR_FINAL_VALUE_MAX_LEN - "Bearer ".len());
        assert!(CredentialInjector::create(&config(CredentialInjectionTarget::Bearer, Some(&exact))).is_some());
    }

    #[tokio::test]
    async fn removes_client_value_then_defers_bearer() {
        let plugin = CredentialInjector::create(&config(CredentialInjectionTarget::Bearer, Some("secret"))).unwrap();
        let mut session = crate::plugins::runtime::traits::session::MockPluginSession::new();
        session
            .expect_is_remote_ip_header()
            .withf(|name| name == "authorization")
            .return_const(false);
        session
            .expect_request_header_values()
            .withf(|name| name == "connection")
            .return_const(Some(Vec::<String>::new()));
        session
            .expect_remove_request_header()
            .withf(|name| name == "authorization")
            .returning(|_| Ok(()));
        session
            .expect_defer_credential_header()
            .withf(|name, value| {
                name == "authorization"
                    && value.as_bytes() == b"Bearer secret"
                    && value.is_sensitive()
                    && format!("{value:?}") == "Sensitive"
            })
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new("CredentialInjector");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        ));
        assert!(!format!("{log:?}").contains("secret"));
    }

    #[tokio::test]
    async fn custom_header_uses_static_prefix() {
        let target = CredentialInjectionTarget::Header {
            name: "X-Provider-Token".to_string(),
            value_prefix: "Token ".to_string(),
        };
        let plugin = CredentialInjector::create(&config(target, Some("secret"))).unwrap();
        let mut session = crate::plugins::runtime::traits::session::MockPluginSession::new();
        session.expect_is_remote_ip_header().return_const(false);
        session
            .expect_request_header_values()
            .return_const(Some(Vec::<String>::new()));
        session.expect_remove_request_header().returning(|_| Ok(()));
        session
            .expect_defer_credential_header()
            .withf(|name, value| {
                name == "x-provider-token"
                    && value.as_bytes() == b"Token secret"
                    && value.is_sensitive()
                    && format!("{value:?}") == "Sensitive"
            })
            .returning(|_, _| Ok(()));

        let mut log = PluginLog::new("CredentialInjector");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        ));
    }

    #[tokio::test]
    async fn rejects_dynamic_connection_and_remote_ip_targets() {
        let target = CredentialInjectionTarget::Header {
            name: "X-Provider-Token".to_string(),
            value_prefix: String::new(),
        };

        let plugin = CredentialInjector::create(&config(target.clone(), Some("secret"))).unwrap();
        let mut session = crate::plugins::runtime::traits::session::MockPluginSession::new();
        session.expect_is_remote_ip_header().return_const(false);
        session
            .expect_request_header_values()
            .return_const(Some(vec!["keep-alive, X-Provider-Token".to_string()]));
        let mut log = PluginLog::new("CredentialInjector");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        ));

        let plugin = CredentialInjector::create(&config(target, Some("secret"))).unwrap();
        let mut session = crate::plugins::runtime::traits::session::MockPluginSession::new();
        session.expect_is_remote_ip_header().return_const(true);
        let mut log = PluginLog::new("CredentialInjector");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        ));
    }
}
