use super::*;

pub(crate) fn mask_buf_len_after_call() -> usize {
    MASK_BUF.with(|buf| buf.borrow().len())
}
use parking_lot::{Mutex, MutexGuard};

// Serialize all tests that mutate SENSITIVE_MODE to prevent races.
static MODE_LOCK: Mutex<()> = Mutex::new(());

struct ModeGuard<'a> {
    prev: u8,
    // Held purely for RAII: serializes tests that mutate SENSITIVE_MODE.
    _lock: MutexGuard<'a, ()>,
}
impl Drop for ModeGuard<'_> {
    fn drop(&mut self) {
        SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
    }
}
fn set_mode(mode: u8) -> ModeGuard<'static> {
    let guard = MODE_LOCK.lock();
    let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
    ModeGuard { prev, _lock: guard }
}

#[test]
fn bounded_masking_checks_rendered_bytes_before_wrap_allocation() {
    let huge = "x".repeat(1024 * 1024);
    for mode in [0, 1, 2, 255] {
        let _g = set_mode(mode);
        let expected = match mode {
            0 => "é".to_string(),
            1 => "***".to_string(),
            _ => "<E>é</E>".to_string(),
        };
        assert_eq!(
            apply_sensitive_bounded("é", expected.len(), str::to_owned),
            Some(expected.clone())
        );
        assert_eq!(
            apply_sensitive_bounded("é", expected.len() - 1, |_| panic!("oversized callback")),
            None::<()>
        );
        let capacity = MASK_BUF.with(|buf| buf.borrow().capacity());
        let result = apply_sensitive_bounded(&huge, 512, str::to_owned);
        assert_eq!(result, (mode == 1).then(|| "***".to_string()));
        assert_eq!(MASK_BUF.with(|buf| buf.borrow().capacity()), capacity);
        assert_eq!(mask_buf_len_after_call(), 0);
    }
}

#[test]
fn test_off_passes_through() {
    let _g = set_mode(0);
    let out = apply_sensitive("1.2.3.4", |s| s.to_string());
    assert_eq!(out, "1.2.3.4");
}

#[test]
fn test_replace_uses_default_stars() {
    let _g = set_mode(1);
    let out = apply_sensitive("1.2.3.4", |s| s.to_string());
    assert_eq!(out, "***");
}

#[test]
fn test_wrap_uses_default_tags() {
    let _g = set_mode(2);
    let out = apply_sensitive("1.2.3.4", |s| s.to_string());
    assert_eq!(out, "<E>1.2.3.4</E>");
}

#[test]
fn test_is_enabled_off() {
    let _g = set_mode(0);
    assert!(!is_enabled());
}

#[test]
fn test_is_enabled_replace() {
    let _g = set_mode(1);
    assert!(is_enabled());
}

#[test]
fn test_is_wrap_off() {
    let _g = set_mode(SensitiveMode::Off as u8);
    assert!(!is_wrap());
}

#[test]
fn test_is_wrap_replace() {
    let _g = set_mode(SensitiveMode::Replace as u8);
    assert!(!is_wrap());
}

#[test]
fn test_is_wrap_wrap() {
    let _g = set_mode(SensitiveMode::Wrap as u8);
    assert!(is_wrap());
}

#[test]
fn test_wrap_reuses_buffer_across_calls() {
    let _g = set_mode(2);
    let a = apply_sensitive("foo", |s| s.to_string());
    let b = apply_sensitive("bar", |s| s.to_string());
    assert_eq!(a, "<E>foo</E>");
    assert_eq!(b, "<E>bar</E>");
}

#[test]
fn test_serde_masked_off() {
    let _g = set_mode(0);
    let out = apply_sensitive("/api/users/123", |w| w.to_string());
    assert_eq!(out, "/api/users/123");
}

// Verify the wrap-mode buffer is cleared immediately after f returns, so
// sensitive content does not linger in the thread-local until the next call.
#[test]
fn test_wrap_clears_buffer_after_call() {
    let _g = set_mode(2);
    let _ = apply_sensitive("top_secret_value", |s| s.to_string());
    assert_eq!(
        mask_buf_len_after_call(),
        0,
        "wrap-mode buffer must be empty after apply_sensitive returns"
    );
}

#[test]
fn default_mode_is_replace() {
    // Privacy-by-default. `ext` rendering follows this mode, so `off` would
    // make the EdgionGatewayConfig whitelist inert out of the box.
    assert_eq!(SensitiveMaskConfig::default().mode, SensitiveMode::Replace);
}
