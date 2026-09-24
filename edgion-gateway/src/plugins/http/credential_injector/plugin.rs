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
#[path = "../../../../tests/unit/plugins/http/credential_injector/plugin_test.rs"]
mod tests;
