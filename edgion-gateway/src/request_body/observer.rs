//! Bounded, failure-open observation of the canonical proxied request body.
//!
//! This is an internal extension seam, not a second body cache. Observers see
//! borrowed post-mutation bytes synchronously and can neither alter forwarding
//! nor retain the transport `Bytes` object.

#![allow(dead_code, reason = "WP5 installs the seam; WP6 adds its first production observer")]

use crate::ctx::CurrentPluginEntryIdentity;
use crate::plugins::runtime::PluginLog;
use edgion_common::startup::catch_redacted_panic;
use pingora_proxy::RequestBodyEvent;

pub(crate) const MAX_REQUEST_BODY_OBSERVERS: usize = 4;
pub(crate) const MAX_OBSERVER_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OBSERVER_EVENTS: usize = 4_096;
pub(crate) const MAX_OBSERVER_WORK: usize = 262_144;
pub(crate) const MAX_OBSERVER_RETAINED_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverLimits {
    bytes: usize,
    events: usize,
    work: usize,
    retained_bytes: usize,
}

impl ObserverLimits {
    pub(crate) fn new(bytes: usize, events: usize, work: usize, retained_bytes: usize) -> Option<Self> {
        if bytes == 0 || events == 0 || work == 0 || retained_bytes == 0 {
            return None;
        }
        Some(Self {
            bytes: bytes.min(MAX_OBSERVER_BYTES),
            events: events.min(MAX_OBSERVER_EVENTS),
            work: work.min(MAX_OBSERVER_WORK),
            retained_bytes: retained_bytes.min(MAX_OBSERVER_RETAINED_BYTES),
        })
    }
}

impl Default for ObserverLimits {
    fn default() -> Self {
        Self {
            bytes: MAX_OBSERVER_BYTES,
            events: MAX_OBSERVER_EVENTS,
            work: MAX_OBSERVER_WORK,
            retained_bytes: MAX_OBSERVER_RETAINED_BYTES,
        }
    }
}

/// Request-wide host budget. Retry resets semantic state, never these counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverBudget {
    remaining_bytes: usize,
    remaining_events: usize,
    remaining_work: usize,
    exhausted: bool,
}

impl ObserverBudget {
    fn new(limits: ObserverLimits) -> Self {
        Self {
            remaining_bytes: limits.bytes,
            remaining_events: limits.events,
            remaining_work: limits.work,
            exhausted: false,
        }
    }

    /// Charge semantic parser work before performing it.
    #[must_use]
    pub(crate) fn charge_work(&mut self, units: usize) -> bool {
        if self.exhausted || units > self.remaining_work {
            self.latch_exhausted();
            return false;
        }
        self.remaining_work -= units;
        true
    }

    fn charge_callback(&mut self, bytes: usize) -> bool {
        if self.exhausted || bytes > self.remaining_bytes || self.remaining_events == 0 || self.remaining_work == 0 {
            self.latch_exhausted();
            return false;
        }
        self.remaining_bytes -= bytes;
        self.remaining_events -= 1;
        self.remaining_work -= 1;
        true
    }

    fn latch_exhausted(&mut self) {
        self.exhausted = true;
        self.remaining_bytes = 0;
        self.remaining_events = 0;
        self.remaining_work = 0;
    }
}

pub(crate) mod sealed {
    /// Implemented only by trusted observers compiled into the gateway.
    pub trait Sealed {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverControl {
    Continue,
    SemanticReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverTerminal {
    Complete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverInitError;

/// Trusted synchronous request-body observer.
pub(crate) trait RequestBodyObserver: sealed::Sealed + Send + Sync {
    /// Fixed scratch retained by the implementation, excluding the trait box.
    fn retained_bytes(&self) -> usize;

    fn observe(
        &mut self,
        bytes: &[u8],
        event: RequestBodyEvent,
        budget: &mut ObserverBudget,
    ) -> Result<ObserverControl, ObserverError>;

    fn reset_for_attempt(&mut self, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        Ok(())
    }

    fn finalize(&mut self, _terminal: ObserverTerminal, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        Ok(())
    }
}

pub(crate) type RequestBodyObserverFactory =
    Box<dyn FnOnce() -> Result<Box<dyn RequestBodyObserver>, ObserverInitError> + Send>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverOutcome {
    Complete,
    Incomplete,
    Exhausted,
    Failed,
    Panicked,
}

impl ObserverOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Exhausted => "exhausted",
            Self::Failed => "failed",
            Self::Panicked => "panicked",
        }
    }
}

struct ObserverSlot {
    observer: Option<Box<dyn RequestBodyObserver>>,
    log: Option<PluginLog>,
    budget: ObserverBudget,
    retained_limit: usize,
    last_attempt: Option<usize>,
    observed_event: bool,
    semantic_ready: bool,
    outcome: Option<ObserverOutcome>,
}

impl ObserverSlot {
    fn check_retained(&mut self) -> bool {
        let Some(observer) = self.observer.as_ref() else {
            return true;
        };
        match catch_redacted_panic(|| observer.retained_bytes()) {
            Ok(bytes) if bytes <= self.retained_limit => true,
            Ok(_) => {
                self.finish_diagnostic(ObserverOutcome::Exhausted);
                false
            }
            Err(_) => {
                self.finish_diagnostic(ObserverOutcome::Panicked);
                false
            }
        }
    }

    fn protected_drop_observer(&mut self) -> bool {
        let Some(observer) = self.observer.take() else {
            return true;
        };
        catch_redacted_panic(|| drop(observer)).is_ok()
    }

    fn finish_diagnostic(&mut self, outcome: ObserverOutcome) {
        if self.outcome.is_some() {
            return;
        }
        if outcome == ObserverOutcome::Exhausted {
            self.budget.latch_exhausted();
        }
        self.outcome = Some(if self.protected_drop_observer() {
            outcome
        } else {
            ObserverOutcome::Panicked
        });
    }

    fn invoke_reset(&mut self) -> bool {
        if !self.budget.charge_callback(0) {
            self.finish_diagnostic(ObserverOutcome::Exhausted);
            return false;
        }
        let result = {
            let observer = self.observer.as_mut().expect("active slot has observer");
            catch_redacted_panic(|| observer.reset_for_attempt(&mut self.budget))
        };
        match result {
            Err(_) => self.finish_diagnostic(ObserverOutcome::Panicked),
            Ok(Err(_)) => self.finish_diagnostic(ObserverOutcome::Failed),
            Ok(Ok(())) if self.budget.exhausted => self.finish_diagnostic(ObserverOutcome::Exhausted),
            Ok(Ok(())) => {
                self.semantic_ready = false;
                if self.check_retained() {
                    return true;
                }
            }
        }
        false
    }

    fn finalize(&mut self, terminal: ObserverTerminal) {
        if self.outcome.is_some() {
            return;
        }
        if !self.budget.charge_callback(0) {
            self.finish_diagnostic(ObserverOutcome::Exhausted);
            return;
        }
        let result = {
            let observer = self.observer.as_mut().expect("active slot has observer");
            catch_redacted_panic(|| observer.finalize(terminal, &mut self.budget))
        };
        match result {
            Err(_) => self.finish_diagnostic(ObserverOutcome::Panicked),
            Ok(Err(_)) => self.finish_diagnostic(ObserverOutcome::Failed),
            Ok(Ok(())) if self.budget.exhausted => self.finish_diagnostic(ObserverOutcome::Exhausted),
            Ok(Ok(())) if !self.check_retained() => {}
            Ok(Ok(())) => {
                let desired = match terminal {
                    ObserverTerminal::Complete => ObserverOutcome::Complete,
                    ObserverTerminal::Incomplete => ObserverOutcome::Incomplete,
                };
                if self.protected_drop_observer() {
                    self.outcome = Some(desired);
                } else {
                    self.outcome = Some(ObserverOutcome::Panicked);
                }
            }
        }
    }

    fn dispatch(&mut self, bytes: &[u8], event: RequestBodyEvent, attempt: usize) {
        if self.outcome.is_some() {
            return;
        }

        if self.observed_event && self.last_attempt != Some(attempt) && !self.invoke_reset() {
            return;
        }
        self.last_attempt = Some(attempt);

        // Semantic readiness may skip later data-only events, but terminal
        // event bytes are always delivered before finalization. A transport
        // can attach the last bytes to Complete/Abandoned.
        if !self.semantic_ready || event.is_terminal() {
            if !self.budget.charge_callback(bytes.len()) {
                self.finish_diagnostic(ObserverOutcome::Exhausted);
                return;
            }
            self.observed_event = true;
            let result = {
                let observer = self.observer.as_mut().expect("active slot has observer");
                catch_redacted_panic(|| observer.observe(bytes, event, &mut self.budget))
            };
            match result {
                Err(_) => {
                    self.finish_diagnostic(ObserverOutcome::Panicked);
                    return;
                }
                Ok(Err(_)) => {
                    self.finish_diagnostic(ObserverOutcome::Failed);
                    return;
                }
                Ok(Ok(_)) if self.budget.exhausted => {
                    self.finish_diagnostic(ObserverOutcome::Exhausted);
                    return;
                }
                Ok(Ok(ObserverControl::SemanticReady)) => self.semantic_ready = true,
                Ok(Ok(ObserverControl::Continue)) => {}
            }
            if !self.check_retained() {
                return;
            }
        }

        match event {
            RequestBodyEvent::Complete => self.finalize(ObserverTerminal::Complete),
            RequestBodyEvent::Abandoned => self.finalize(ObserverTerminal::Incomplete),
            RequestBodyEvent::Data => {}
        }
    }
}

impl Drop for ObserverSlot {
    fn drop(&mut self) {
        let _ = self.protected_drop_observer();
    }
}

/// Fixed-capacity registry allocated only after an observer registration is attempted.
pub(crate) struct RequestBodyObservers {
    slots: [Option<ObserverSlot>; MAX_REQUEST_BODY_OBSERVERS],
    len: usize,
}

impl Default for RequestBodyObservers {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

impl RequestBodyObservers {
    pub(crate) fn is_full(&self) -> bool {
        self.len == MAX_REQUEST_BODY_OBSERVERS
    }

    pub(crate) fn try_install(
        &mut self,
        identity: CurrentPluginEntryIdentity,
        factory: RequestBodyObserverFactory,
        limits: ObserverLimits,
    ) -> bool {
        if self.is_full() {
            let _ = catch_redacted_panic(|| drop(factory));
            return false;
        }

        let Ok(Ok(observer)) = catch_redacted_panic(factory) else {
            return false;
        };
        let Ok(retained) = catch_redacted_panic(|| observer.retained_bytes()) else {
            let _ = catch_redacted_panic(|| drop(observer));
            return false;
        };
        if retained > limits.retained_bytes {
            let _ = catch_redacted_panic(|| drop(observer));
            return false;
        }

        self.slots[self.len] = Some(ObserverSlot {
            observer: Some(observer),
            log: Some(PluginLog::with_alias(identity.name(), identity.alias())),
            budget: ObserverBudget::new(limits),
            retained_limit: limits.retained_bytes,
            last_attempt: None,
            observed_event: false,
            semantic_ready: false,
            outcome: None,
        });
        self.len += 1;
        true
    }

    /// Observe a transport event without copying or mutating its bytes.
    pub(crate) fn observe(&mut self, bytes: &[u8], event: RequestBodyEvent, attempt: usize) {
        for slot in self.slots[..self.len].iter_mut().flatten() {
            slot.dispatch(bytes, event, attempt);
        }
    }

    /// Idempotently finish active slots and transfer their bounded logs.
    pub(crate) fn finalize_incomplete_and_take_logs(&mut self) -> Vec<PluginLog> {
        let mut logs = Vec::with_capacity(self.len);
        for slot in self.slots[..self.len].iter_mut().flatten() {
            slot.finalize(ObserverTerminal::Incomplete);
            if let (Some(mut log), Some(outcome)) = (slot.log.take(), slot.outcome) {
                log.push(outcome.label());
                logs.push(log);
            }
        }
        logs
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/observer_test.rs"]
mod tests;
