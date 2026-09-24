#[cfg(test)]
pub(crate) struct GlobalStoreTestLock(parking_lot::Mutex<()>);
