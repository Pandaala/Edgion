/// Sticky per-thread lookup fault for deterministic decode/panic coverage.
/// Cleared by [`LookupFaultGuard`] on drop so a panicked test cannot leak into
/// the next one sharing the thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupFault {
    /// Return `Err(MaxMindDbError::Decoding)` from inside the lookup closure so
    /// the production `Ok(Err(_)) => Failed` arm is the path under test.
    Decode,
    /// Panic inside the same `catch_unwind` used on the request path.
    Panic,
}
