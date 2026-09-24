/// Serializes cross-module global-store tests and restores the degrade default
/// and overrides on both normal return and unwind.
#[cfg(test)]
pub(crate) static GLOBAL_STORE_TEST_LOCK: GlobalStoreTestLock = GlobalStoreTestLock(parking_lot::Mutex::new(()));
