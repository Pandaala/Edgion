#[cfg(test)]
impl Drop for LookupFaultGuard {
    fn drop(&mut self) {
        LOOKUP_FAULT.with(|c| c.set(None));
    }
}
