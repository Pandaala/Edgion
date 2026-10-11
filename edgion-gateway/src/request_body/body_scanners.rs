//! Generic digest scanners over raw request-body bytes.

use hmac::{Hmac, Mac};
use md5::Digest as _;

use crate::request_body::BodyReadError;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl};

pub(crate) struct Md5Scanner(md5::Md5);

impl Md5Scanner {
    pub(crate) fn new() -> Self {
        Self(md5::Md5::new())
    }
}

impl RawBodyScanner for Md5Scanner {
    type Output = [u8; 16];

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.0.update(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.0.finalize().into())
    }
}

pub(crate) struct HmacSha256Scanner(Hmac<sha2::Sha256>);

impl HmacSha256Scanner {
    pub(crate) fn new(key: &[u8]) -> Result<Self, BodyOperationError> {
        Hmac::<sha2::Sha256>::new_from_slice(key)
            .map(Self)
            .map_err(|_| BodyOperationError::Body(BodyReadError::Incomplete))
    }
}

impl RawBodyScanner for HmacSha256Scanner {
    type Output = [u8; 32];

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        self.0.update(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(self) -> Result<Self::Output, BodyOperationError> {
        Ok(self.0.finalize().into_bytes().into())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/body_scanners_test/digest_scanner_tests.rs"]
mod digest_scanner_tests;
