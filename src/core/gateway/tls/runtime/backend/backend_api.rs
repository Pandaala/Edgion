use crate::types::resources::edgion_tls::EdgionTls;
use pingora_core::tls::ssl::{SslRef, SslVerifyMode};
use std::sync::Arc;

#[inline]
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String> {
    crate::core::gateway::tls::boringssl::mtls_verify_callback::set_verify_callback_with_whitelist(
        ssl,
        verify_mode,
        edgion_tls,
    )
}
