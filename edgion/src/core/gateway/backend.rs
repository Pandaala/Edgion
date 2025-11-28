//! Backend selection utilities
//!
//! Provides functions to select backend references using load balancing algorithms.

use std::sync::Arc;
use pingora_core::{Error as PingoraError, ErrorType};
use pingora_proxy::Session;
use crate::core::lb::{ERR_NO_BACKEND_REFS, ERR_INCONSISTENT_WEIGHT};
use crate::core::gateway::edgion_http_context::EdgionHttpContext;
use crate::core::gateway::end_response_500;
use crate::types::{EdgionErrStatus, HTTPBackendRef, HTTPRouteRule};

/// Select a backend reference using the cached selector from the matched route.
/// 
/// On first call, initializes the selector; subsequent calls reuse the cached selector.
pub async fn select_backend_ref(
    session: &mut Session,
    ctx: &mut EdgionHttpContext,
    matched_route: &Arc<HTTPRouteRule>,
) -> pingora_core::Result<HTTPBackendRef> {
    // Initialize selector if not yet initialized
    if !matched_route.backend_finder.is_initialized() {
        let (items, weights) = match &matched_route.backend_refs {
            Some(refs) if !refs.is_empty() => {
                let items: Vec<HTTPBackendRef> = refs.clone();
                let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight).collect();
                (items, weights)
            }
            _ => (vec![], vec![]),
        };
        matched_route.backend_finder.init(items, weights);
    }

    // Select backend
    match matched_route.backend_finder.select() {
        Ok(backend) => Ok(backend),
        Err(err_code) => {
            let err_status = match err_code {
                ERR_NO_BACKEND_REFS => EdgionErrStatus::UpstreamNotBackendRefs,
                ERR_INCONSISTENT_WEIGHT => EdgionErrStatus::UpstreamInconsistentWeight,
                _ => EdgionErrStatus::UpstreamNotBackendRefs,
            };
            tracing::error!("Failed to select backend: {:?}", err_status);
            ctx.add_error(err_status);
            end_response_500(session).await?;
            Err(PingoraError::new(ErrorType::InternalError))
        }
    }
}
