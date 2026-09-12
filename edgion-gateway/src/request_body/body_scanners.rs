//! Generic digest scanners over raw request-body bytes.

use hmac::{Hmac, Mac};
use md5::Digest as _;

use crate::plugins::runtime::traits::session::BodyReadError;
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
mod digest_scanner_tests {
    use super::*;
    use crate::request_body::RequestBodyState;

    #[tokio::test]
    async fn known_empty_body_finishes_digest_scanners_without_scan_bytes() {
        let mut body = RequestBodyState::default();
        body.mark_known_empty();

        let md5 = body.scan_raw(Md5Scanner::new(), 0).await.unwrap();
        assert_eq!(hex::encode(md5.value), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5.bytes_scanned, 0);

        let hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 0)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(hmac.value),
            "f9e66e179b6747ae54108f82f8ade8b3c25d76fd30afde6c395822c530196169"
        );
        assert_eq!(hmac.bytes_scanned, 0);
    }

    #[tokio::test]
    async fn digest_scanners_follow_the_published_mutation_snapshot() {
        use bytes::Bytes;
        use pingora_core::protocols::http::RequestBodyBuffer;

        let (mut storage, view) = crate::request_body::BodyStorage::new(1, 3);
        storage.write(&Bytes::from_static(b"abc")).await.unwrap();
        storage.finish().await.unwrap();
        let mut body = RequestBodyState::default();
        body.install(view);

        let initial_md5 = body.scan_raw(Md5Scanner::new(), 3).await.unwrap();
        assert_eq!(hex::encode(initial_md5.value), "900150983cd24fb0d6963f7d28e17f72");
        let initial_hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 3)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(initial_hmac.value),
            "9946dad4e00e913fc8be8e5d3f7e110a4a9e832f83fb09c345285d78638d8a0e"
        );

        body.replace_literal(Bytes::from_static(b"a"), Bytes::from_static(b"d"), 3)
            .await
            .unwrap();
        let mutated_md5 = body.scan_raw(Md5Scanner::new(), 3).await.unwrap();
        assert_eq!(hex::encode(mutated_md5.value), "4a429e0b8871ec788a3a171545d959e8");
        let mutated_hmac = body
            .scan_raw(HmacSha256Scanner::new(b"secret").unwrap(), 3)
            .await
            .unwrap();
        assert_eq!(
            hex::encode(mutated_hmac.value),
            "1f24ebf1762bcdba36c3dc8fcebb4b0acc409ac6531ddda81136effbdd6bb52a"
        );
    }
}
