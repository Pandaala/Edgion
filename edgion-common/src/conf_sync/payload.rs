//! Shared bounded payload encoding for configuration List and Watch responses.

use std::{fmt, io::Read};

use bytes::Bytes;
use tokio::sync::Semaphore;

use super::proto::PayloadEncoding;

pub const COMPRESSION_THRESHOLD: usize = 32 * 1024 * 1024;
/// A transport safety budget, independent of per-resource validation limits.
pub const MAX_PAYLOAD_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_MESSAGE_BYTES: usize = MAX_PAYLOAD_BYTES + 64 * 1024;
const MAX_WINDOW_LOG: u32 = 27;
static CODEC_JOBS: Semaphore = Semaphore::const_new(2);

#[derive(Clone)]
pub struct EncodedPayload {
    pub data: Bytes,
    pub encoding: i32,
    pub uncompressed_size: u64,
}

impl fmt::Debug for EncodedPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedPayload")
            .field("wire_size", &self.data.len())
            .field("encoding", &self.encoding)
            .field("uncompressed_size", &self.uncompressed_size)
            .finish()
    }
}

/// Errors intentionally exclude payload contents and dependency error strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    TooLarge,
    UnknownEncoding,
    LengthMismatch,
    InvalidCompressedData,
    CodecUnavailable,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "configuration payload exceeds the 512 MiB transport safety budget",
            Self::UnknownEncoding => "unsupported configuration payload encoding",
            Self::LengthMismatch => "configuration payload length does not match its declared size",
            Self::InvalidCompressedData => "invalid compressed configuration payload",
            Self::CodecUnavailable => "configuration payload codec unavailable",
        })
    }
}

impl std::error::Error for PayloadError {}

impl PayloadError {
    pub fn into_status(self) -> tonic::Status {
        match self {
            Self::TooLarge => tonic::Status::resource_exhausted(self.to_string()),
            Self::CodecUnavailable => tonic::Status::internal(self.to_string()),
            _ => tonic::Status::data_loss(self.to_string()),
        }
    }
}

/// Small payloads retain their original allocation; large compression runs off-runtime.
pub async fn encode(data: Bytes) -> Result<EncodedPayload, PayloadError> {
    if data.len() > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge);
    }
    if data.len() <= COMPRESSION_THRESHOLD {
        return Ok(identity(data));
    }
    let permit = CODEC_JOBS.acquire().await.map_err(|_| PayloadError::CodecUnavailable)?;
    tokio::task::spawn_blocking(move || {
        // Cancellation of the waiting future must not release this slot early.
        let _permit = permit;
        compress(data)
    })
    .await
    .map_err(|_| PayloadError::CodecUnavailable)?
}

fn identity(data: Bytes) -> EncodedPayload {
    EncodedPayload {
        uncompressed_size: data.len() as u64,
        data,
        encoding: PayloadEncoding::Identity as i32,
    }
}

fn compress(data: Bytes) -> Result<EncodedPayload, PayloadError> {
    let compressed = zstd::stream::encode_all(data.as_ref(), 3).map_err(|_| PayloadError::CodecUnavailable)?;
    if compressed.len() >= data.len() {
        return Ok(identity(data));
    }
    Ok(EncodedPayload {
        uncompressed_size: data.len() as u64,
        data: compressed.into(),
        encoding: PayloadEncoding::Zstd as i32,
    })
}

/// Validate before exposing bytes to JSON deserialization or cache publication.
pub async fn decode(data: Bytes, encoding: i32, uncompressed_size: u64) -> Result<Bytes, PayloadError> {
    if data.len() > MAX_PAYLOAD_BYTES || uncompressed_size > MAX_PAYLOAD_BYTES as u64 {
        return Err(PayloadError::TooLarge);
    }
    let encoding = PayloadEncoding::try_from(encoding).map_err(|_| PayloadError::UnknownEncoding)?;
    let expected = uncompressed_size as usize;
    match encoding {
        PayloadEncoding::Identity => {
            if data.len() != expected {
                return Err(PayloadError::LengthMismatch);
            }
            Ok(data)
        }
        PayloadEncoding::Zstd => {
            let permit = CODEC_JOBS.acquire().await.map_err(|_| PayloadError::CodecUnavailable)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                decompress(data, expected)
            })
            .await
            .map_err(|_| PayloadError::CodecUnavailable)?
        }
    }
}

fn decompress(data: Bytes, expected: usize) -> Result<Bytes, PayloadError> {
    let mut decoder = zstd::stream::read::Decoder::with_buffer(data.as_ref())
        .map_err(|_| PayloadError::InvalidCompressedData)?
        .single_frame();
    decoder
        .window_log_max(MAX_WINDOW_LOG)
        .map_err(|_| PayloadError::InvalidCompressedData)?;
    // Do not reserve a peer-controlled declared size before validating the frame.
    let mut output = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let count = decoder
            .read(&mut chunk)
            .map_err(|_| PayloadError::InvalidCompressedData)?;
        if count == 0 {
            break;
        }
        if count > expected.saturating_sub(output.len()) {
            return Err(PayloadError::LengthMismatch);
        }
        output.extend_from_slice(&chunk[..count]);
    }
    // Exactly one complete frame is permitted; no trailing bytes or second frame.
    if !decoder.finish().is_empty() {
        return Err(PayloadError::InvalidCompressedData);
    }
    if output.len() != expected {
        return Err(PayloadError::LengthMismatch);
    }
    Ok(output.into())
}

#[cfg(test)]
#[path = "../../tests/unit/conf_sync/payload_test.rs"]
mod tests;
