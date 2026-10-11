//! Canonical request-body read contracts shared by storage and consumers.

use async_trait::async_trait;
use bytes::Bytes;

/// Why a body could not be read.
///
/// The body gate applies plugin failure policy only to recoverable read
/// failures. Admission refusals and the gateway-wide physical buffer limit are
/// always fail-close because the original body is unavailable for forwarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyReadError {
    /// The operation is unavailable for the selected adapter or configuration.
    /// Production request-body capture is implemented; aggregation of the main
    /// upstream response is intentionally unsupported because it stays streaming.
    Unimplemented,
    /// The gateway **declined to buffer** this body (protocol or stage
    /// refusal — see the body-gate admission model). This is NOT evidence that
    /// the request had no body, and it is never softened by
    /// `BodyRequirement::on_read_failure`.
    NotBuffered,
    /// The body exceeded the gateway-wide physical buffer limit. During
    /// capture, the partial body is discarded and the error always maps to
    /// HTTP 413. During an atomic mutation, the previous complete version
    /// remains available and the mutating plugin's failure policy applies.
    BufferLimitExceeded { limit: u64 },
    /// The complete body was buffered, but exceeds a plugin's own logical
    /// inspection limit. Because the forwarding copy still exists, the
    /// plugin's configured fail-open / fail-close policy can apply.
    TooLarge { limit: u64 },
    /// The body stream ended prematurely / was aborted.
    Incomplete,
    /// A mutation was computed against one logical snapshot but another
    /// mutation was published before it could be applied. Kept distinct from
    /// `Incomplete` so a lost update is never mistaken for a truncated body:
    /// the two demand opposite handling, and byte offsets from the stale
    /// snapshot must never be spliced into the newer one.
    GenerationConflict,
    /// The capture storage backend failed (spill file create/write, or the
    /// capture was poisoned mid-chunk): an infrastructure fault, not a client
    /// abort. Kept distinct from `Incomplete` so operators can alert on it,
    /// and always fail-close — the consumed bytes were not stored, so no
    /// forwarding copy of the body exists.
    StorageFailure,
    /// Downstream request-body capture exceeded its time bound. Distinct from
    /// `Incomplete` (an aborted stream) and `StorageFailure` (storage I/O): a
    /// capture deadline expiry is a client error and maps to HTTP 408, not 500.
    Timeout,
}

impl std::fmt::Display for BodyReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyReadError::Unimplemented => write!(f, "body buffering not implemented"),
            BodyReadError::NotBuffered => write!(f, "body not buffered"),
            BodyReadError::BufferLimitExceeded { limit } => {
                write!(f, "body exceeds gateway buffer limit of {limit} bytes")
            }
            BodyReadError::TooLarge { limit } => write!(f, "body exceeds buffer cap of {limit} bytes"),
            BodyReadError::Incomplete => write!(f, "body stream incomplete"),
            BodyReadError::GenerationConflict => write!(f, "body mutation conflict"),
            BodyReadError::StorageFailure => write!(f, "body capture storage failed"),
            BodyReadError::Timeout => write!(f, "body read timed out"),
        }
    }
}

impl std::error::Error for BodyReadError {}

/// Result of reading a buffered body: the bytes (a cheap `Arc`-backed handle, so
/// cloning is a refcount bump, not a copy), or the reason it is unavailable.
pub type BodyReadResult = Result<Bytes, BodyReadError>;

/// A synchronous borrowed consumer driven only after canonical byte admission.
/// Inspection failures are latched by the consumer; capture still proves EOF and
/// preserves transport/storage error priority. Chunks cannot escape this call.
pub trait RequestBodyChunkConsumer: Send {
    /// Optional observation window. A canonical complete snapshot may skip its
    /// remaining bytes after proving its full length fits the hard read cap.
    fn source_window(&self) -> Option<u64> {
        None
    }

    /// Opt in to one bounded, already-resident contiguous snapshot chunk.
    /// Live capture remains chunked. This is an input bound, not a yield guarantee.
    fn cached_contiguous_limit(&self) -> Option<usize> {
        None
    }

    fn consume(&mut self, chunk: &[u8]);
}

/// An owned reader over one immutable logical request-body snapshot.
///
/// Each returned chunk is bounded by the body backend. The reader owns every
/// backing reference needed to outlive later body mutations, so consumers may
/// safely stream it across await points without borrowing the plugin session.
#[async_trait]
pub trait RequestBodySnapshotReader: Send {
    /// Exact byte length of this snapshot.
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Take the complete unread snapshot as a shared owning memory slice, only
    /// when already contiguous, nonempty and at most `max_bytes`. Never copy,
    /// concatenate or read disk to satisfy this capability. Success advances
    /// this reader to its end; callers must still verify EOF with `next_chunk`.
    /// Unavailable sources return `None` without advancing. Invalid ranges return
    /// an error without advancing. An isolated reader must keep its isolated owner.
    fn try_take_contiguous_memory(&mut self, _max_bytes: usize) -> Result<Option<Bytes>, BodyReadError> {
        Ok(None)
    }

    /// Bytes that must be copied to isolate this reader's in-memory segments.
    ///
    /// This is the size of the prospective independent payload, not the size
    /// of the original backing allocations. Callers must reserve this budget
    /// before calling `isolate_memory` and retain it for the isolated data's
    /// entire lifetime, including any returned chunks that share its backing.
    /// `None` means isolation is unsupported or the size cannot be represented.
    fn memory_bytes_to_isolate(&self) -> Option<usize> {
        None
    }

    /// Synchronously detach in-memory segments into independently owned bytes.
    ///
    /// Only supported before reading starts. Disk segments remain streamed
    /// from their existing storage; this never materializes disk-backed data.
    fn isolate_memory(&mut self) -> Result<(), BodyReadError> {
        Err(BodyReadError::Unimplemented)
    }

    /// Read the next raw byte chunk, or `None` after the complete snapshot.
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError>;
}
