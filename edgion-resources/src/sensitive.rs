use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

// ---- global state ----

// 0=Off, 1=Replace, 2=Wrap
pub static SENSITIVE_MODE: AtomicU8 = AtomicU8::new(0);
static SENSITIVE_STRINGS: OnceLock<SensitiveStrings> = OnceLock::new();

struct SensitiveStrings {
    replace_with: String,
    wrap_prefix: String,
    wrap_suffix: String,
}

impl Default for SensitiveStrings {
    fn default() -> Self {
        Self {
            replace_with: "***".to_string(),
            wrap_prefix: "<E>".to_string(),
            wrap_suffix: "</E>".to_string(),
        }
    }
}

fn get_strings() -> &'static SensitiveStrings {
    SENSITIVE_STRINGS.get_or_init(SensitiveStrings::default)
}

// ---- public config types ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SensitiveMode {
    Off = 0,
    #[default]
    Replace = 1,
    Wrap = 2,
}

fn default_replace_with() -> String {
    "***".to_string()
}
fn default_wrap_prefix() -> String {
    "<E>".to_string()
}
fn default_wrap_suffix() -> String {
    "</E>".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SensitiveMaskConfig {
    pub mode: SensitiveMode,
    #[serde(default = "default_replace_with")]
    pub replace_with: String,
    #[serde(default = "default_wrap_prefix")]
    pub wrap_prefix: String,
    #[serde(default = "default_wrap_suffix")]
    pub wrap_suffix: String,
}

impl Default for SensitiveMaskConfig {
    fn default() -> Self {
        Self {
            mode: SensitiveMode::Replace,
            replace_with: default_replace_with(),
            wrap_prefix: default_wrap_prefix(),
            wrap_suffix: default_wrap_suffix(),
        }
    }
}

/// Initialize once at gateway startup. Subsequent calls are no-ops (OnceLock).
///
/// Uses `Ordering::Release` so any thread that later observes `SENSITIVE_MODE != 0`
/// via an `Acquire` load is guaranteed to also see the `SENSITIVE_STRINGS` write.
pub fn init_sensitive(config: SensitiveMaskConfig) {
    let _ = SENSITIVE_STRINGS.set(SensitiveStrings {
        replace_with: config.replace_with,
        wrap_prefix: config.wrap_prefix,
        wrap_suffix: config.wrap_suffix,
    });
    SENSITIVE_MODE.store(config.mode as u8, Ordering::Release);
}

#[inline]
pub fn is_enabled() -> bool {
    SENSITIVE_MODE.load(Ordering::Acquire) != 0
}

#[inline]
pub fn is_wrap() -> bool {
    SENSITIVE_MODE.load(Ordering::Acquire) == SensitiveMode::Wrap as u8
}

/// Returns the total byte overhead added by Wrap mode (`prefix.len() + suffix.len()`).
/// Returns 0 when mode is Off or Replace.
pub fn wrap_overhead() -> usize {
    if SENSITIVE_MODE.load(Ordering::Acquire) != 2 {
        return 0;
    }
    let s = get_strings();
    s.wrap_prefix.len() + s.wrap_suffix.len()
}

// ---- thread-local buffer (avoids heap allocation in wrap mode) ----

thread_local! {
    static MASK_BUF: RefCell<String> = RefCell::new(String::with_capacity(256));
}

/// Apply current masking mode to `v` and pass the result to `f`.
/// - Off: passes `v` unchanged
/// - Replace: passes `replace_with` string
/// - Wrap: passes `<prefix>v<suffix>` using a thread-local buffer (no heap alloc)
///
/// # Non-reentrant in Wrap mode
/// The Wrap branch holds a `RefCell` borrow for the duration of `f`. Calling
/// `apply_sensitive` again inside `f` (directly or via serde) will panic with
/// "already mutably borrowed". All current callers satisfy this constraint.
/// If nested calls become necessary, replace `MASK_BUF` with a stack-allocated
/// `String` (one heap alloc per call, no thread-local needed).
#[inline]
pub fn apply_sensitive<T>(v: &str, f: impl FnOnce(&str) -> T) -> T {
    apply_sensitive_inner(v, None, f).expect("unbounded masking always invokes the callback")
}

/// Apply masking only when the complete rendered value fits `max_bytes`.
/// Rejection does not invoke `f` or grow the Wrap-mode thread-local buffer.
/// The same non-reentrancy constraint as [`apply_sensitive`] applies in Wrap mode.
#[inline]
pub fn apply_sensitive_bounded<T>(v: &str, max_bytes: usize, f: impl FnOnce(&str) -> T) -> Option<T> {
    apply_sensitive_inner(v, Some(max_bytes), f)
}

#[inline]
fn apply_sensitive_inner<T>(v: &str, max_bytes: Option<usize>, f: impl FnOnce(&str) -> T) -> Option<T> {
    match SENSITIVE_MODE.load(Ordering::Acquire) {
        0 => {
            if max_bytes.is_some_and(|max| v.len() > max) {
                return None;
            }
            Some(f(v))
        }
        1 => {
            let replacement = &get_strings().replace_with;
            if max_bytes.is_some_and(|max| replacement.len() > max) {
                return None;
            }
            Some(f(replacement))
        }
        _ => {
            let s = get_strings();
            if let Some(max) = max_bytes {
                let len = s
                    .wrap_prefix
                    .len()
                    .checked_add(v.len())?
                    .checked_add(s.wrap_suffix.len())?;
                if len > max {
                    return None;
                }
            }
            Some(MASK_BUF.with(|buf| {
                let mut buf = buf
                    .try_borrow_mut()
                    .expect("apply_sensitive is not re-entrant in Wrap mode");
                buf.clear();
                buf.push_str(&s.wrap_prefix);
                buf.push_str(v);
                buf.push_str(&s.wrap_suffix);
                let result = f(&buf);
                buf.clear();
                result
            }))
        }
    }
}

// ---- test-only helpers ----

#[cfg(test)]
pub(crate) fn mask_buf_len_after_call() -> usize {
    MASK_BUF.with(|buf| buf.borrow().len())
}

// ---- serde helper modules ----

/// For `String` fields: `#[serde(serialize_with = "crate::sensitive::masked::serialize")]`
pub mod masked {
    use serde::Serializer;
    pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        super::apply_sensitive(v, |w| s.serialize_str(w))
    }
}

/// For `Option<String>` fields: `#[serde(serialize_with = "crate::sensitive::masked_opt::serialize")]`
pub mod masked_opt {
    use serde::Serializer;
    pub fn serialize<S: Serializer>(v: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => super::masked::serialize(inner.as_str(), s),
        }
    }
}

/// For `Vec<String>` fields: `#[serde(serialize_with = "crate::sensitive::masked_vec::serialize")]`
pub mod masked_vec {
    use serde::ser::SerializeSeq;
    use serde::Serializer;
    pub fn serialize<S: Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for item in v {
            super::apply_sensitive(item, |w| seq.serialize_element(w))?;
        }
        seq.end()
    }
}

// ---- tests ----
#[cfg(test)]
mod tests {
    use super::*;
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
}
