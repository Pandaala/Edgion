/// Exclusive guard for tests that mutate the process-global AI store.
///
/// Drop always clears the store, including during unwinding after a failed
/// assertion, so one test cannot leak resources into another.
#[cfg(test)]
pub struct GlobalAiBackendTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
}
