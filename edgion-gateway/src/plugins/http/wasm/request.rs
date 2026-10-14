//! Request-owned guest execution. Only short ownership transfers lock; guest
//! callbacks and network waits run with an exclusive loan outside that lock.

use super::vm_manager::VmLease;
use parking_lot::Mutex;
use std::sync::Arc;

pub struct WasmRequestBinding {
    pub(crate) identity: Arc<()>,
    state: Mutex<BindingState>,
}

// Allocate the execution once per admitted request. Each streamed callback
// transfers a pointer rather than copying the Store and cached export handles.
enum BindingState {
    Active(Box<Execution>),
    Borrowed,
    Closed,
}

pub(crate) struct Execution {
    pub lease: VmLease,
    pub response_ended: bool,
    pub response_headers_seen: bool,
}

pub(crate) struct ExecutionLoan {
    owner: Arc<WasmRequestBinding>,
    execution: Option<Box<Execution>>,
}

impl WasmRequestBinding {
    pub(crate) fn new(identity: Arc<()>, lease: VmLease) -> Arc<Self> {
        Arc::new(Self {
            identity,
            state: Mutex::new(BindingState::Active(Box::new(Execution {
                lease,
                response_ended: false,
                response_headers_seen: false,
            }))),
        })
    }

    pub(crate) fn take(self: &Arc<Self>) -> Option<ExecutionLoan> {
        let mut state = self.state.lock();
        match std::mem::replace(&mut *state, BindingState::Borrowed) {
            BindingState::Active(execution) => Some(ExecutionLoan {
                owner: self.clone(),
                execution: Some(execution),
            }),
            previous => {
                *state = previous;
                None
            }
        }
    }

    /// Cancellation never re-enters a guest. A borrowed execution observes the
    /// closed state when it tries to return, and is discarded as well.
    pub(crate) fn abort(&self) {
        let previous = std::mem::replace(&mut *self.state.lock(), BindingState::Closed);
        if let BindingState::Active(mut execution) = previous {
            execution.lease.poison();
        }
    }

    /// The universal request terminal hook is the sole normal cleanup owner.
    pub(crate) fn finish(self: &Arc<Self>) -> bool {
        let Some(mut loan) = self.take() else {
            let previous = std::mem::replace(&mut *self.state.lock(), BindingState::Closed);
            return matches!(previous, BindingState::Closed);
        };
        let execution = loan.execution();
        execution.lease.state_mut().frame.read_only = true;
        let ok = execution
            .lease
            .context_id()
            .is_some_and(|id| execution.lease.close_context(id).is_ok());
        let execution = loan.execution.take();
        *self.state.lock() = BindingState::Closed;
        drop(execution);
        ok
    }
}

impl ExecutionLoan {
    pub(crate) fn execution(&mut self) -> &mut Execution {
        self.execution.as_mut().expect("live execution loan")
    }

    pub(crate) fn complete(mut self) {
        let mut execution = self.execution.take().expect("live execution loan");
        execution.lease.state_mut().release_phase_payloads();
        if !execution.lease.is_healthy() {
            self.owner.abort();
            return;
        }
        let mut state = self.owner.state.lock();
        if matches!(*state, BindingState::Borrowed) {
            *state = BindingState::Active(execution);
        } else {
            drop(state);
            execution.lease.poison();
        }
    }
}

impl Drop for ExecutionLoan {
    fn drop(&mut self) {
        if let Some(mut execution) = self.execution.take() {
            self.owner.abort();
            execution.lease.poison();
        }
    }
}

/// Empty requests allocate nothing. A hard bound prevents a configuration with
/// many entries from accumulating an unbounded owner list on a single request.
#[derive(Default)]
pub(crate) struct WasmRequestState {
    bindings: Vec<Arc<WasmRequestBinding>>,
}

impl WasmRequestState {
    pub(crate) fn install(&mut self, binding: Arc<WasmRequestBinding>) -> bool {
        if self.bindings.len() >= 64 || self.get(&binding.identity).is_some() {
            binding.abort();
            return false;
        }
        self.bindings.push(binding);
        true
    }

    pub(crate) fn get(&self, identity: &Arc<()>) -> Option<Arc<WasmRequestBinding>> {
        self.bindings
            .iter()
            .find(|binding| Arc::ptr_eq(&binding.identity, identity))
            .cloned()
    }

    pub(crate) fn finish(&mut self) -> bool {
        let mut ok = true;
        for binding in self.bindings.drain(..) {
            ok &= binding.finish();
        }
        ok
    }
}

impl Drop for WasmRequestState {
    fn drop(&mut self) {
        for binding in &self.bindings {
            binding.abort();
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/request_test.rs"]
mod tests;
