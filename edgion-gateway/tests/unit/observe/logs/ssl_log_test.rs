use super::TlsConnMeta;
use std::cell::RefCell;

thread_local! {
    static ENTRIES: RefCell<Option<Vec<TlsConnMeta>>> = const { RefCell::new(None) };
}

pub(crate) fn record(entry: &TlsConnMeta) {
    ENTRIES.with(|entries| {
        if let Some(entries) = entries.borrow_mut().as_mut() {
            entries.push(entry.clone());
        }
    });
}

pub(crate) fn capture(f: impl FnOnce()) -> Vec<TlsConnMeta> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ENTRIES.with(|entries| *entries.borrow_mut() = None);
        }
    }
    ENTRIES.with(|entries| {
        assert!(entries.borrow().is_none());
        *entries.borrow_mut() = Some(Vec::new());
    });
    let _reset = Reset;
    f();
    ENTRIES.with(|entries| entries.borrow_mut().take().unwrap())
}
