//! Request-local ownership for writable response processors.
//!
//! The driver is deliberately Edgion-owned: Pingora continues to own protocol
//! pumping and cache ordering, while this type owns the product processor set.
//! A short-lived execution lease distinguishes a normally returned callback
//! from cancellation of the callback future. Cancellation releases only the
//! processor group whose callback was in flight; the request-local driver
//! remains reachable for logging and shared-owner finalization.

use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard};

use crate::plugins::runtime::traits::{StreamingResponseProcessor, UpstreamResponseTrailer};

pub(crate) struct ResponseProcessorDriver {
    state: Mutex<ResponseProcessorDriverState>,
    has_body: bool,
    body_changes_length: bool,
    #[cfg(test)]
    ordinary_count: usize,
    trailer_count: usize,
}

struct ResponseProcessorDriverState {
    body: BodyProcessorSet,
    trailers: Vec<Box<dyn UpstreamResponseTrailer>>,
    body_callback_cancelled: bool,
    trailer_callback_cancelled: bool,
}

pub(crate) struct BodyProcessorSet {
    pub(crate) ai: Option<Box<dyn StreamingResponseProcessor>>,
    pub(crate) semantic: Option<Box<dyn StreamingResponseProcessor>>,
    pub(crate) ordinary: Vec<Box<dyn StreamingResponseProcessor>>,
}

impl BodyProcessorSet {
    fn is_empty(&self) -> bool {
        self.ai.is_none() && self.semantic.is_none() && self.ordinary.is_empty()
    }

    fn changes_body_length(&self) -> bool {
        self.ai
            .as_ref()
            .is_some_and(|processor| processor.changes_body_length())
            || self
                .semantic
                .as_ref()
                .is_some_and(|processor| processor.changes_body_length())
            || self.ordinary.iter().any(|processor| processor.changes_body_length())
    }

    fn release_inflight(&mut self) {
        if let Some(processor) = self.ai.as_mut() {
            processor.release_inflight();
        }
        if let Some(processor) = self.semantic.as_mut() {
            processor.release_inflight();
        }
        for processor in &mut self.ordinary {
            processor.release_inflight();
        }
    }
}

impl ResponseProcessorDriver {
    pub(crate) fn new(
        ai: Option<Box<dyn StreamingResponseProcessor>>,
        semantic: Option<Box<dyn StreamingResponseProcessor>>,
        ordinary: Vec<Box<dyn StreamingResponseProcessor>>,
        trailers: Vec<Box<dyn UpstreamResponseTrailer>>,
    ) -> Option<Arc<Self>> {
        let body = BodyProcessorSet { ai, semantic, ordinary };
        if body.is_empty() && trailers.is_empty() {
            return None;
        }
        let has_body = !body.is_empty();
        let body_changes_length = body.changes_body_length();
        #[cfg(test)]
        let ordinary_count = body.ordinary.len();
        let trailer_count = trailers.len();
        Some(Arc::new(Self {
            state: Mutex::new(ResponseProcessorDriverState {
                body,
                trailers,
                body_callback_cancelled: false,
                trailer_callback_cancelled: false,
            }),
            has_body,
            body_changes_length,
            #[cfg(test)]
            ordinary_count,
            trailer_count,
        }))
    }

    #[inline]
    pub(crate) fn has_body_processors(&self) -> bool {
        self.has_body
    }

    #[inline]
    pub(crate) fn body_changes_length(&self) -> bool {
        self.body_changes_length
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn ordinary_count(&self) -> usize {
        self.ordinary_count
    }

    #[inline]
    pub(crate) fn trailer_count(&self) -> usize {
        self.trailer_count
    }

    pub(crate) async fn body_lease(&self) -> Option<BodyExecutionLease<'_>> {
        let guard = self.state.lock().await;
        if guard.body_callback_cancelled {
            return None;
        }
        Some(BodyExecutionLease {
            guard,
            completed: false,
        })
    }

    pub(crate) async fn trailer_lease(&self) -> Option<TrailerExecutionLease<'_>> {
        let guard = self.state.lock().await;
        if guard.trailer_callback_cancelled {
            return None;
        }
        Some(TrailerExecutionLease {
            guard,
            completed: false,
        })
    }

    pub(crate) async fn finalize_ai_incomplete(&self) {
        let mut state = self.state.lock().await;
        if let Some(processor) = state.body.ai.as_mut() {
            processor.finalize_incomplete("terminal", "logging-fallback");
        }
    }

    pub(crate) async fn finalize_semantic_incomplete(&self, outcome: &'static str, reason: &'static str) {
        let mut state = self.state.lock().await;
        if state.body_callback_cancelled {
            return;
        }
        if let Some(processor) = state.body.semantic.as_mut() {
            processor.finalize_incomplete(outcome, reason);
        }
    }

    pub(crate) async fn finalize_trailers_incomplete(&self) {
        let mut state = self.state.lock().await;
        if state.trailer_callback_cancelled {
            return;
        }
        for companion in &mut state.trailers {
            companion.finalize_incomplete("logging-fallback");
        }
    }
}

pub(crate) struct BodyExecutionLease<'a> {
    guard: MutexGuard<'a, ResponseProcessorDriverState>,
    completed: bool,
}

impl BodyExecutionLease<'_> {
    pub(crate) fn processors(&mut self) -> &mut BodyProcessorSet {
        &mut self.guard.body
    }

    pub(crate) fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for BodyExecutionLease<'_> {
    fn drop(&mut self) {
        if self.completed || self.guard.body_callback_cancelled {
            return;
        }
        self.guard.body.release_inflight();
        self.guard.body_callback_cancelled = true;
    }
}

pub(crate) struct TrailerExecutionLease<'a> {
    guard: MutexGuard<'a, ResponseProcessorDriverState>,
    completed: bool,
}

impl TrailerExecutionLease<'_> {
    pub(crate) fn companions(&mut self) -> &mut Vec<Box<dyn UpstreamResponseTrailer>> {
        &mut self.guard.trailers
    }

    pub(crate) fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for TrailerExecutionLease<'_> {
    fn drop(&mut self) {
        if self.completed || self.guard.trailer_callback_cancelled {
            return;
        }
        for companion in &mut self.guard.trailers {
            companion.release_inflight();
        }
        self.guard.trailer_callback_cancelled = true;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/response/driver_test.rs"]
mod tests;
