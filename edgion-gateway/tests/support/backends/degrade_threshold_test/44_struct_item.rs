#[cfg(test)]
pub(crate) struct GlobalStoreTestGuard<'a> {
    // Fields drop in declaration order: restore while the shared lock is held.
    _state: DegradeTestState,
    _lock: parking_lot::MutexGuard<'a, ()>,
}
