#[cfg(test)]
pub(crate) static LINK_SYS_CAP_TEST_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());
