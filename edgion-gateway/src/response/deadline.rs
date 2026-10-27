//! Absolute deadline selection for bounded response owners.
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeadlineSource {
    Hold,
    Request,
    Grpc,
}
#[derive(Clone, Copy)]
pub(crate) struct EffectiveDeadline {
    pub(crate) at: Instant,
    pub(crate) source: DeadlineSource,
}
pub(crate) fn effective_deadline(
    now: Instant,
    hold: std::time::Duration,
    request: Option<Instant>,
    grpc: Option<Instant>,
) -> Option<EffectiveDeadline> {
    let mut selected = EffectiveDeadline {
        at: now.checked_add(hold)?,
        source: DeadlineSource::Hold,
    };
    // Request wins equal deadlines, then gRPC, then the converter ceiling.
    if let Some(at) = grpc.filter(|at| *at <= selected.at) {
        selected = EffectiveDeadline {
            at,
            source: DeadlineSource::Grpc,
        };
    }
    if let Some(at) = request.filter(|at| *at <= selected.at) {
        selected = EffectiveDeadline {
            at,
            source: DeadlineSource::Request,
        };
    }
    Some(selected)
}
