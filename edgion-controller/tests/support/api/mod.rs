use super::*;

#[cfg(test)]
pub(crate) fn reset_lifecycle_terminated_for_test() {
    LIFECYCLE_TERMINATED.store(false, std::sync::atomic::Ordering::Release);
}
