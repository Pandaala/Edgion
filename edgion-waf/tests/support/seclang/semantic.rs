std::thread_local! {
    pub(super) static VALIDATION_ALLOCATION_FAILURE: std::cell::Cell<Option<&'static str>> = const {
        std::cell::Cell::new(None)
    };
}
