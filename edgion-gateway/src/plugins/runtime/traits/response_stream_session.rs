//! Transport-free capabilities available while a response callback is awaited.

use crate::ai::protocols::openai::canonical_sse::CanonicalRecordBatch;
use crate::ai::protocols::openai::terminal::AiClientStreamDialect;
use crate::link_sys::template::TrustedTemplateVariables;
use crate::response::terminal::{FinalResponseEligibility, ResponseTerminalCause, TerminalEmission};
use edgion_resources::EdgionErrorCode;

use super::PluginSession;

/// A response callback may mutate request state, but cannot borrow transport I/O.
/// This leaves the downstream session available for canonical close detection.
pub trait ResponseStreamSession: TrustedTemplateVariables {
    fn record_error(&mut self, code: EdgionErrorCode);
    fn try_claim_response_terminal(&mut self, cause: ResponseTerminalCause) -> bool;
    fn mark_response_terminal_emission(&mut self, emission: TerminalEmission);
    fn semantic_guard_active(&mut self) -> bool;
    fn publish_canonical_records(&mut self, batch: CanonicalRecordBatch) -> Result<(), CanonicalRecordBatch>;
    fn take_canonical_records(&mut self) -> Option<CanonicalRecordBatch>;
    fn ai_terminal_render_context(&mut self) -> Option<(AiClientStreamDialect, FinalResponseEligibility)>;
}

// Request-stage helpers and concrete alternate adapters retain their real
// session behavior when used through the smaller response capability.
impl<T: PluginSession + ?Sized> ResponseStreamSession for T {
    fn record_error(&mut self, code: EdgionErrorCode) {
        PluginSession::record_error(self, code);
    }
    fn try_claim_response_terminal(&mut self, cause: ResponseTerminalCause) -> bool {
        PluginSession::try_claim_response_terminal(self, cause)
    }
    fn mark_response_terminal_emission(&mut self, emission: TerminalEmission) {
        PluginSession::mark_response_terminal_emission(self, emission);
    }
    fn semantic_guard_active(&mut self) -> bool {
        self.ai().semantic_guard_active()
    }
    fn publish_canonical_records(&mut self, batch: CanonicalRecordBatch) -> Result<(), CanonicalRecordBatch> {
        self.ai().publish_canonical_records(batch)
    }
    fn take_canonical_records(&mut self) -> Option<CanonicalRecordBatch> {
        self.ai().take_canonical_records()
    }
    fn ai_terminal_render_context(&mut self) -> Option<(AiClientStreamDialect, FinalResponseEligibility)> {
        self.ai().ai_terminal_render_context()
    }
}

/// Trailer companions need committed framing facts, never live transport I/O.
pub trait ResponseTrailerSession: Send + Sync {
    fn record_error(&mut self, code: EdgionErrorCode);
    fn is_grpc_request(&self) -> bool;
    fn downstream_response_trailers_representable(&self) -> bool;
    fn downstream_response_trailer_name_allowed(&self, name: &http::HeaderName) -> bool;
}

impl<T: PluginSession + ?Sized> ResponseTrailerSession for T {
    fn record_error(&mut self, code: EdgionErrorCode) {
        PluginSession::record_error(self, code);
    }
    fn is_grpc_request(&self) -> bool {
        PluginSession::is_grpc_request(self)
    }
    fn downstream_response_trailers_representable(&self) -> bool {
        PluginSession::downstream_response_trailers_representable(self)
    }
    fn downstream_response_trailer_name_allowed(&self, name: &http::HeaderName) -> bool {
        PluginSession::downstream_response_trailer_name_allowed(self, name)
    }
}
