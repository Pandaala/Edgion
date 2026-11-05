//! Serialize complete JSON responses under a byte cap before exposing any body.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::io::{self, Write};

pub(super) const MAX_JSON_BYTES: usize = 10 * 1024 * 1024;
const MAX_PROJECTION_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROJECTION_ROWS: usize = 1000;

#[derive(Debug)]
pub struct BoundedJson<T>(pub T);

#[derive(Debug)]
struct CappedWriter {
    bytes: Option<Vec<u8>>,
    length: usize,
    limit: usize,
    exceeded: bool,
}
impl Write for CappedWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.limit - self.length {
            self.exceeded = true;
            return Err(io::Error::other("JSON output limit exceeded"));
        }
        if let Some(bytes) = &mut self.bytes {
            if bytes.capacity() - bytes.len() < input.len() {
                // Grow in bounded chunks, never geometrically past the output cap.
                let wanted = (self.length + input.len()).saturating_add(8191) / 8192 * 8192;
                bytes.reserve_exact(wanted.min(self.limit) - bytes.len());
            }
            bytes.extend_from_slice(input);
        }
        self.length += input.len();
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn serialize<T: Serialize + ?Sized>(value: &T, limit: usize, collect: bool) -> Result<CappedWriter, StatusCode> {
    let mut writer = CappedWriter {
        bytes: collect.then(Vec::new),
        length: 0,
        limit,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.exceeded {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        });
    }
    Ok(writer)
}
impl<T: Serialize> IntoResponse for BoundedJson<T> {
    fn into_response(self) -> Response {
        match serialize(&self.0, MAX_JSON_BYTES, true) {
            Ok(writer) => (
                [(http::header::CONTENT_TYPE, "application/json")],
                writer.bytes.unwrap_or_default(),
            )
                .into_response(),
            Err(status) => (
                status,
                if status == StatusCode::PAYLOAD_TOO_LARGE {
                    "JSON response exceeds limit"
                } else {
                    "JSON response serialization failed"
                },
            )
                .into_response(),
        }
    }
}

/// A shared projection ledger counts serialized bytes and all nested rows before
/// retaining them. Its headroom covers enclosing response fields and separators.
pub(super) struct ProjectionBudget {
    remaining: usize,
    rows: usize,
}
impl ProjectionBudget {
    pub(super) fn new() -> Self {
        Self {
            remaining: MAX_PROJECTION_BYTES,
            rows: 0,
        }
    }
    pub(super) fn reserve<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), StatusCode> {
        if self.rows >= MAX_PROJECTION_ROWS {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        let measured = serialize(value, self.remaining, false)?.length;
        self.remaining = self
            .remaining
            .checked_sub(measured + 1)
            .ok_or(StatusCode::PAYLOAD_TOO_LARGE)?;
        self.rows += 1;
        Ok(())
    }
}

pub(super) fn check<T: Serialize + ?Sized>(value: &T) -> Result<(), StatusCode> {
    serialize(value, MAX_JSON_BYTES, false).map(|_| ())
}

#[cfg(test)]
#[path = "../../tests/unit/api/bounded_json_test.rs"]
mod tests;
