use super::*;

#[cfg(test)]
pub(super) static TEST_SERVICE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(super) fn lock_test_service() -> parking_lot::MutexGuard<'static, ()> {
    TEST_SERVICE_LOCK.lock()
}

#[cfg(test)]
pub(super) fn install_test_generation() -> u64 {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let task = tokio::spawn(std::future::pending::<()>());
    *SERVICE.lock() = Some(ServiceHandle {
        generation,
        abort: task.abort_handle(),
    });
    generation
}
