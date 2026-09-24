/// Inject a sticky lookup fault until the returned guard is dropped.
#[cfg(test)]
pub fn inject_lookup_fault(fault: LookupFault) -> LookupFaultGuard {
    LOOKUP_FAULT.with(|c| c.set(Some(fault)));
    LookupFaultGuard
}
