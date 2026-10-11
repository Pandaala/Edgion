//! Selected-WAF operations at the plugin/session integration boundary.

use crate::ctx::{WafErrorCode, WafLogPhase, WafPhase, WafPhaseClaim};
use crate::request_body::BodyReadError;
use edgion_waf::seclang::{PhaseOutcome, Transaction};
use std::sync::Arc;

/// Short-lived access to the selected request's WAF state and native admission.
///
/// Implementations preserve the request context as the sole state owner. All
/// methods are explicit: an unavailable adapter must not silently claim a phase
/// or enable streaming. Borrow this capability for one operation at a time.
#[cfg_attr(test, mockall::automock)]
#[cfg_attr(test, allow(clippy::disallowed_types))]
pub trait WafSessionCapability: Send + Sync {
    /// Stop remaining phases while retaining independent hard admission.
    fn stop_inspection(&mut self, owner: &Arc<()>, bytes: u64, storage_limit: bool) -> bool;

    /// Install hard admission and freeze mutation before checking native and
    /// cached lengths. Admission outlives transaction take or removal.
    fn install_body_limit(&mut self, owner: &Arc<()>, limit: u64) -> Result<(), BodyReadError>;

    /// Consume the offered transaction unconditionally. Success transfers it to
    /// the selected context. Rejection drops only this offer, preserves any
    /// resident transaction, and requires the caller's terminal failure path.
    fn install_transaction(&mut self, owner: Arc<()>, transaction: Transaction<'static>) -> bool;

    /// Transfer the matching owner's transaction to the callback temporarily.
    fn take_transaction(&mut self, owner: &Arc<()>) -> Option<Transaction<'static>>;

    /// Claim a selected phase once; missing election must fail closed.
    fn claim_phase(&mut self, owner: &Arc<()>, phase: WafPhase) -> WafPhaseClaim;

    /// Append a completed phase to the request-owned bounded summary.
    fn record_phase_outcome(&mut self, owner: &Arc<()>, phase: WafLogPhase, execution_us: u64, outcome: &PhaseOutcome);

    /// Retain the first terminal error as a closed, non-sensitive code.
    fn record_terminal_error(&mut self, owner: &Arc<()>, phase: WafLogPhase, code: WafErrorCode, status: u16);

    /// Activate the selected direct body consumer and permanently inhibit retry.
    fn enable_request_body_streaming(&mut self, owner: &Arc<()>, body_limit: u64) -> bool;
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/traits/waf_session_test.rs"]
mod tests;
