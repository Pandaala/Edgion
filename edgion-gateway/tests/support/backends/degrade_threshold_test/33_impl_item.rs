#[cfg(test)]
impl GlobalStoreTestLock {
    pub(crate) fn lock(&self) -> GlobalStoreTestGuard<'_> {
        let lock = self.0.lock();
        GlobalStoreTestGuard {
            _state: DegradeTestState::capture(),
            _lock: lock,
        }
    }
}
