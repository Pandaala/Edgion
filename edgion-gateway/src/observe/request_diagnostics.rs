use edgion_resources::{EdgionErrorCode, EdgionInfoCode};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use smallvec::SmallVec;

pub const INLINE_REQUEST_DIAGNOSTICS: usize = 8;
pub const MAX_REQUEST_DIAGNOSTICS: usize = 32;

/// One intrinsically classified request diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDiagnostic {
    Info(EdgionInfoCode),
    Error(EdgionErrorCode),
}

impl Serialize for RequestDiagnostic {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RequestDiagnostic", 2)?;
        match self {
            Self::Info(code) => {
                state.serialize_field("kind", "info")?;
                state.serialize_field("code", code)?;
            }
            Self::Error(code) => {
                state.serialize_field("kind", "error")?;
                state.serialize_field("code", code)?;
            }
        }
        state.end()
    }
}

/// Ordered, bounded request diagnostics with an allocation-free common path.
#[derive(Debug, Clone, Default)]
pub struct RequestDiagnostics {
    entries: SmallVec<[RequestDiagnostic; INLINE_REQUEST_DIAGNOSTICS]>,
    full: bool,
}

impl RequestDiagnostics {
    #[inline]
    pub fn record_info(&mut self, code: EdgionInfoCode) -> bool {
        self.push(RequestDiagnostic::Info(code))
    }

    #[inline]
    pub fn record_error(&mut self, code: EdgionErrorCode) -> bool {
        self.push(RequestDiagnostic::Error(code))
    }

    #[inline]
    fn push(&mut self, diagnostic: RequestDiagnostic) -> bool {
        if self.entries.len() >= MAX_REQUEST_DIAGNOSTICS {
            self.full = true;
            return false;
        }
        if self.entries.len() == INLINE_REQUEST_DIAGNOSTICS && self.entries.capacity() < MAX_REQUEST_DIAGNOSTICS {
            self.entries.reserve_exact(MAX_REQUEST_DIAGNOSTICS - self.entries.len());
        }
        self.entries.push(diagnostic);
        true
    }

    #[inline]
    pub fn entries(&self) -> &[RequestDiagnostic] {
        self.entries.as_slice()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        self.full
    }

    #[inline]
    pub fn contains_error(&self, code: EdgionErrorCode) -> bool {
        self.entries
            .iter()
            .any(|entry| *entry == RequestDiagnostic::Error(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_entries_preserve_order_and_repeats() {
        let mut diagnostics = RequestDiagnostics::default();
        diagnostics.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);
        diagnostics.record_info(EdgionInfoCode::PluginResponseReplaced);
        diagnostics.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);

        assert_eq!(
            diagnostics.entries(),
            &[
                RequestDiagnostic::Error(EdgionErrorCode::BackendRequestTimeoutRetry),
                RequestDiagnostic::Info(EdgionInfoCode::PluginResponseReplaced),
                RequestDiagnostic::Error(EdgionErrorCode::BackendRequestTimeoutRetry),
            ]
        );
        assert_eq!(
            serde_json::to_value(diagnostics.entries()).unwrap(),
            serde_json::json!([
                {"kind": "error", "code": "BackendRequestTimeoutRetry"},
                {"kind": "info", "code": "PluginResponseReplaced"},
                {"kind": "error", "code": "BackendRequestTimeoutRetry"}
            ])
        );
    }

    #[test]
    fn storage_spills_once_and_drops_only_after_the_hard_limit() {
        let mut diagnostics = RequestDiagnostics::default();
        for _ in 0..INLINE_REQUEST_DIAGNOSTICS {
            assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
        }
        assert!(!diagnostics.entries.spilled());

        assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
        assert!(diagnostics.entries.spilled());
        let spilled_capacity = diagnostics.entries.capacity();
        assert!(spilled_capacity >= MAX_REQUEST_DIAGNOSTICS);

        while diagnostics.entries().len() < MAX_REQUEST_DIAGNOSTICS {
            assert!(diagnostics.record_error(EdgionErrorCode::Unknown));
            assert_eq!(diagnostics.entries.capacity(), spilled_capacity);
        }
        assert!(!diagnostics.is_full());
        assert!(!diagnostics.record_error(EdgionErrorCode::ClientClosed));
        assert!(diagnostics.is_full());
        assert_eq!(diagnostics.entries().len(), MAX_REQUEST_DIAGNOSTICS);
        assert!(!diagnostics.contains_error(EdgionErrorCode::ClientClosed));
    }
}
