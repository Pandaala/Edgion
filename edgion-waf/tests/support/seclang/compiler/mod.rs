thread_local! {
    pub(super) static COMPILER_ALLOCATION_FAILURE: std::cell::Cell<Option<&'static str>> = const {
        std::cell::Cell::new(None)
    };
}
