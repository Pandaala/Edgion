//! Bounded background tasks owned by a plugin instance.

use std::future::Future;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::task::AbortHandle;

struct State<const N: usize> {
    active: bool,
    tasks: [Option<AbortHandle>; N],
}

/// The unique owner closes the scope even when tasks retain handles to it.
pub(crate) struct PluginTaskOwner<const N: usize> {
    handle: PluginTaskHandle<N>,
}

#[derive(Clone)]
pub(crate) struct PluginTaskHandle<const N: usize> {
    state: Arc<Mutex<State<N>>>,
}

impl<const N: usize> PluginTaskOwner<N> {
    pub(crate) fn new() -> Self {
        Self {
            handle: PluginTaskHandle {
                state: Arc::new(Mutex::new(State {
                    active: true,
                    tasks: std::array::from_fn(|_| None),
                })),
            },
        }
    }

    pub(crate) fn handle(&self) -> PluginTaskHandle<N> {
        self.handle.clone()
    }

    pub(crate) fn close(&self) {
        let tasks = {
            let mut state = self.handle.state.lock();
            state.active = false;
            std::mem::replace(&mut state.tasks, std::array::from_fn(|_| None))
        };
        for task in tasks.into_iter().flatten() {
            task.abort();
        }
    }
}

impl<const N: usize> Drop for PluginTaskOwner<N> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<const N: usize> PluginTaskHandle<N> {
    /// Run a short publication operation atomically against owner shutdown.
    /// The callback must not await or recursively enter this scope.
    pub(crate) fn with_active<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        let state = self.state.lock();
        state.active.then(operation)
    }

    /// Each caller owns a fixed slot. A completed task may be replaced.
    pub(crate) fn spawn(&self, slot: usize, future: impl Future<Output = ()> + Send + 'static) -> bool {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let mut state = self.state.lock();
        if !state.active {
            return false;
        }
        let Some(task) = state.tasks.get_mut(slot) else {
            return false;
        };
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return false;
        }
        *task = Some(runtime.spawn(future).abort_handle());
        true
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/task_scope_test.rs"]
mod tests;
