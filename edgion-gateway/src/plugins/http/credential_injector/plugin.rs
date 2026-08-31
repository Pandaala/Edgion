//! Secret-backed upstream credential injection.
//!
//! The request-stage plugin removes any client-supplied value immediately, but
//! defers the resolved credential until the final attempt-local upstream header
//! is built. This keeps credential material out of mirrors and downstream
//! request state while preserving ordered plugin override semantics.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use bytes::BytesMut;
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
    valid_until: Option<Instant>,
}

fn sensitive_header_value(raw_value: BytesMut) -> Result<HeaderValue, http::header::InvalidHeaderValue> {
    let mut value = HeaderValue::from_maybe_shared(raw_value.freeze())?;
    value.set_sensitive(true);
    Ok(value)
}

impl CredentialInjector {
    pub fn create(config: &CredentialInjectorConfig) -> Option<Box<dyn RequestFilter>> {
        let resolved = config.resolved_credential.as_ref()?;
        let valid_until = match resolved.valid_until.as_deref() {
            None => None,
            Some(raw) => {
                let wall_now = SystemTime::now();
                let monotonic_now = Instant::now();
                let remaining = edgion_common::time::parse_rfc3339_deadline(raw, wall_now)?;
                if remaining.is_zero() {
                    return None;
                }
                Some(monotonic_now.checked_add(remaining)?)
            }
        };
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
        let mut raw_value = BytesMut::with_capacity(final_len);
        raw_value.extend_from_slice(prefix.as_bytes());
        raw_value.extend_from_slice(resolved.value.as_bytes());
        let value = Arc::new(sensitive_header_value(raw_value).ok()?);

        Some(Box::new(Self {
            name: "CredentialInjector",
            target,
            value,
            valid_until,
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
        if self.valid_until.is_some_and(|deadline| Instant::now() >= deadline) {
            return Self::reject(log, "credential expired");
        }
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
            .defer_credential_header(self.target.clone(), Arc::clone(&self.value), self.valid_until)
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
                valid_until: None,
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

    #[test]
    fn sensitive_header_value_reuses_the_input_buffer() {
        let mut raw_value = BytesMut::with_capacity(32);
        raw_value.extend_from_slice(b"Bearer private-token");
        let input_ptr = raw_value.as_ptr();

        let value = sensitive_header_value(raw_value).unwrap();

        assert_eq!(value.as_bytes().as_ptr(), input_ptr);
        assert_eq!(value.as_bytes(), b"Bearer private-token");
        assert!(value.is_sensitive());
        assert_eq!(format!("{value:?}"), "Sensitive");
    }

    #[test]
    fn construction_rejects_invalid_or_expired_managed_deadlines() {
        let mut invalid = config(CredentialInjectionTarget::Bearer, Some("secret"));
        invalid.resolved_credential.as_mut().unwrap().valid_until = Some("not-a-timestamp".to_string());
        assert!(CredentialInjector::create(&invalid).is_none());

        let mut expired = config(CredentialInjectionTarget::Bearer, Some("secret"));
        expired.resolved_credential.as_mut().unwrap().valid_until = Some("1970-01-01T00:00:01Z".to_string());
        assert!(CredentialInjector::create(&expired).is_none());
    }

    #[tokio::test]
    async fn expired_runtime_generation_fails_before_request_mutation() {
        let plugin = CredentialInjector {
            name: "CredentialInjector",
            target: HeaderName::from_static("authorization"),
            value: Arc::new(HeaderValue::from_static("Bearer secret")),
            valid_until: Some(Instant::now() - std::time::Duration::from_secs(1)),
        };
        let mut session = crate::plugins::runtime::traits::session::MockPluginSession::new();
        let mut log = PluginLog::new("CredentialInjector");
        assert!(matches!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        ));
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
            .withf(|name, value, valid_until| {
                name == "authorization"
                    && value.as_bytes() == b"Bearer secret"
                    && value.is_sensitive()
                    && format!("{value:?}") == "Sensitive"
                    && valid_until.is_none()
            })
            .returning(|_, _, _| Ok(()));

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
            .withf(|name, value, valid_until| {
                name == "x-provider-token"
                    && value.as_bytes() == b"Token secret"
                    && value.is_sensitive()
                    && format!("{value:?}") == "Sensitive"
                    && valid_until.is_none()
            })
            .returning(|_, _, _| Ok(()));

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
