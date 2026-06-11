//! Mandatory secret redaction for the gateway admin `/configclient` output.
//!
//! Unlike `crate::sensitive` (operator-configurable access-log masking),
//! this is an always-on security invariant with no config knob. A gateway admin
//! handler constructs an [`AdminRedactGuard`] around its synchronous serialization;
//! while the guard is alive on the current thread, the `redact_opt` / `redact_required`
//! serde adapters emit `"[redacted]"` instead of the real value.
//!
//! Default (no guard) emits the real value, so the controller's conf_sync transport
//! serialization — which runs in a different process and never constructs the guard —
//! is unaffected. See docs/superpowers/specs/2026-06-07-gateway-admin-api-harden-design.md.
//!
//! INVARIANT: the guard is a thread-local, so it only applies on the thread that runs
//! `serde_json::to_value`. Never move the admin serialization off that thread
//! (no rayon/`par_iter`/`spawn_blocking` in the `/configclient` serialize path), and
//! never add an overriding `impl Serialize` / `#[serde(into)]` on a type ABOVE a
//! secret field — either would silently bypass the field adapters.

use std::cell::Cell;

use serde::{Serialize, Serializer};

thread_local! {
    static REDACT_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// True while an [`AdminRedactGuard`] is alive on the current thread.
#[inline]
pub fn redact_active() -> bool {
    REDACT_ACTIVE.with(|c| c.get())
}

/// RAII guard that turns on mandatory secret redaction for the current thread.
///
/// Construct it immediately before the synchronous serialization of admin output
/// (no `.await` may occur while it is held). Re-entrancy safe: it saves and restores
/// the previous flag value, and `Drop` runs on panic unwinding, so a panicking
/// handler cannot leave redaction stuck on for the next request on the same thread.
pub struct AdminRedactGuard {
    prev: bool,
}

impl AdminRedactGuard {
    // `new()` has the side effect of activating the redact scope, so a `Default`
    // impl would be misleading; suppress the lint here on the impl block.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let prev = REDACT_ACTIVE.with(|c| c.replace(true));
        Self { prev }
    }
}

impl Drop for AdminRedactGuard {
    fn drop(&mut self) {
        REDACT_ACTIVE.with(|c| c.set(self.prev));
    }
}

/// serde adapter for `Option<T>` secret fields.
/// `#[serde(serialize_with = "crate::redact::redact_opt::serialize")]`
///
/// In an admin-redact scope, emits the scalar `"[redacted]"` (never recursing into
/// `T`, so the whole subtree collapses). Otherwise serializes the real value.
pub mod redact_opt {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

/// serde adapter for bare (non-`Option`) secret fields, e.g. `CsrfConfig.key: String`.
/// `#[serde(serialize_with = "crate::redact::redact_required::serialize")]`
pub mod redact_required {
    use super::{redact_active, Serialize, Serializer};

    pub fn serialize<T, S>(v: &T, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if redact_active() {
            s.serialize_str("[redacted]")
        } else {
            v.serialize(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Demo {
        #[serde(skip_serializing_if = "Option::is_none", serialize_with = "redact_opt::serialize")]
        opt_secret: Option<String>,
        #[serde(serialize_with = "redact_required::serialize")]
        bare_secret: String,
        public_field: String,
    }

    fn demo() -> Demo {
        Demo {
            opt_secret: Some("OPT-SECRET".to_string()),
            bare_secret: "BARE-SECRET".to_string(),
            public_field: "visible".to_string(),
        }
    }

    #[test]
    fn redacts_when_guard_active() {
        let json = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(demo()).unwrap()
        };
        assert_eq!(json["opt_secret"], "[redacted]");
        assert_eq!(json["bare_secret"], "[redacted]");
        assert_eq!(json["public_field"], "visible");
        let s = json.to_string();
        assert!(!s.contains("OPT-SECRET"), "opt secret leaked: {s}");
        assert!(!s.contains("BARE-SECRET"), "bare secret leaked: {s}");
    }

    #[test]
    fn emits_real_value_without_guard() {
        let json = serde_json::to_value(demo()).unwrap();
        assert_eq!(json["opt_secret"], "OPT-SECRET");
        assert_eq!(json["bare_secret"], "BARE-SECRET");
    }

    #[test]
    fn none_option_is_omitted_when_active() {
        let _g = AdminRedactGuard::new();
        let json = serde_json::to_value(Demo {
            opt_secret: None,
            bare_secret: "x".to_string(),
            public_field: "p".to_string(),
        })
        .unwrap();
        assert!(json.get("opt_secret").is_none(), "None field should be omitted");
    }

    #[test]
    fn guard_clears_on_drop() {
        assert!(!redact_active());
        {
            let _g = AdminRedactGuard::new();
            assert!(redact_active());
        }
        assert!(!redact_active());
    }
}
