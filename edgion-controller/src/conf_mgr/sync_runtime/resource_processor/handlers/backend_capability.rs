//! Shared raw `EdgionBackend` capability classification for policy handlers.

use crate::conf_mgr::sync_runtime::resource_processor::HandlerContext;
use edgion_resources::prelude_resources::EdgionBackend;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, ResolvedPolicyTargetClass};

/// Capability derived from the current raw backend generation.
///
/// Policy handlers deliberately classify the raw cache rather than a processed
/// generation so a variant or scheme change withdraws stale policy eligibility
/// before the backend worker publishes its resolved runtime proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RawBackendCapability {
    pub target_class: ResolvedPolicyTargetClass,
    pub supports_tls_policy: bool,
}

pub(super) fn classify_raw_backend(
    target: &PolicyTargetKey,
    ctx: &HandlerContext,
) -> Result<Option<RawBackendCapability>, String> {
    let Some(processor) = ctx.runtime().processor_registry.get("EdgionBackend") else {
        return Ok(None);
    };
    let key = format!("{}/{}", target.namespace, target.name);
    let Some(raw) = processor.get_raw_any(&key) else {
        return Ok(None);
    };
    let backend = raw
        .downcast::<EdgionBackend>()
        .map_err(|_| format!("EdgionBackend {key} has an incompatible cached representation"))?;
    backend
        .spec
        .validate()
        .map_err(|_| format!("EdgionBackend {key} has an invalid backend specification"))?;

    if let (Some(ai), None) = (backend.spec.ai.as_ref(), backend.spec.a2a.as_ref()) {
        let endpoint = ai
            .effective_endpoint()
            .map_err(|_| format!("EdgionBackend {key} has an invalid AI endpoint"))?;
        return Ok(Some(RawBackendCapability {
            target_class: ResolvedPolicyTargetClass::Ai,
            supports_tls_policy: endpoint.scheme() == "https",
        }));
    }

    match (backend.spec.ai.as_ref(), backend.spec.a2a.as_ref()) {
        (None, Some(a2a)) if a2a.target.service_ref.is_some() && a2a.target.endpoint.is_none() => {
            Ok(Some(RawBackendCapability {
                target_class: ResolvedPolicyTargetClass::A2aService,
                supports_tls_policy: true,
            }))
        }
        (None, Some(a2a)) if a2a.target.endpoint.is_some() && a2a.target.service_ref.is_none() => {
            let endpoint = a2a
                .normalized_endpoint()
                .map_err(|_| format!("EdgionBackend {key} has an invalid A2A endpoint"))?;
            Ok(Some(RawBackendCapability {
                target_class: ResolvedPolicyTargetClass::A2aExternal,
                supports_tls_policy: endpoint.starts_with("https://"),
            }))
        }
        _ => Err(format!(
            "EdgionBackend {key} does not configure exactly one supported backend variant"
        )),
    }
}
