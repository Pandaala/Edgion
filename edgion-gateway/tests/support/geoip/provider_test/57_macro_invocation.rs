#[cfg(test)]
thread_local! {
    static LOOKUP_FAULT: Cell<Option<LookupFault>> = const { Cell::new(None) };
}
