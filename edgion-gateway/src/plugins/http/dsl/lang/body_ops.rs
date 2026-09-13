//! DSL body policy over the current request state, borrowed for one operation.
//!
//! Storage owns snapshots and scanner scheduling. Mutations always pass through
//! PluginSession, which owns stage restrictions and request replay handoff.

use std::future::Future;
use std::ops::Range;

use bytes::Bytes;

use crate::plugins::runtime::PluginSession;
use crate::request_body::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, BoundedTextResult, HmacSha256Scanner,
    Md5Scanner, RawBodyScanner,
};

use super::super::runtime::DslRuntime;
use super::body_scan_budget::BodyScanBudget;
use super::error::RuntimeError;
use super::value::{BodyValue, Value};
use super::vm::VmLimits;

pub(crate) fn body_access_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::BodyAccess {
        message: message.into(),
    }
}

pub(crate) fn map_body_error(error: BodyOperationError) -> RuntimeError {
    body_access_error(error.to_string())
}

pub(crate) fn expect_body(value: Value, operation: &'static str) -> Result<(), RuntimeError> {
    if matches!(value, Value::Body(BodyValue::Request)) {
        Ok(())
    } else {
        Err(RuntimeError::TypeError {
            expected: "Body",
            got: value.type_name(),
            operation,
        })
    }
}

pub(crate) struct DslBodyOps<'a> {
    session: &'a mut dyn PluginSession,
    budget: &'a mut BodyScanBudget,
    limits: &'a VmLimits,
    runtime: &'a DslRuntime,
}

impl<'a> DslBodyOps<'a> {
    pub(crate) fn new(
        session: &'a mut dyn PluginSession,
        budget: &'a mut BodyScanBudget,
        limits: &'a VmLimits,
        runtime: &'a DslRuntime,
    ) -> Self {
        Self {
            session,
            budget,
            limits,
            runtime,
        }
    }

    fn reserve(&mut self) -> Result<u64, RuntimeError> {
        self.budget.reserve(
            self.session.ctx().request_body_config.max_body_size_bytes,
            self.runtime.max_body_scan_passes,
        )
    }

    fn settle<T>(&mut self, reserved: u64, scan: BodyScanResult<T>) -> T {
        self.budget.settle(reserved, scan.bytes_scanned);
        scan.value
    }

    fn check_string_len(&self, len: usize) -> Result<(), RuntimeError> {
        if len > self.limits.max_string_len {
            return Err(RuntimeError::StringTooLong {
                len,
                limit: self.limits.max_string_len,
            });
        }
        Ok(())
    }

    pub(crate) fn byte_len(&mut self) -> Result<u64, RuntimeError> {
        let known_bodyless = self.session.declares_body() == Some(false);
        self.session
            .request_body_available_len(known_bodyless)
            .map_err(|error| body_access_error(error.to_string()))
    }

    pub(crate) async fn read_text_range(&mut self, offset: u64, max_bytes: u64) -> Result<String, RuntimeError> {
        // Reject the requested length before reserving scan work or clipping
        // to the available body. Lossy output is bounded independently below.
        if max_bytes > self.limits.max_string_len as u64 {
            return Err(RuntimeError::StringTooLong {
                len: usize::try_from(max_bytes).unwrap_or(usize::MAX),
                limit: self.limits.max_string_len,
            });
        }
        let reserved = self.reserve()?;
        let scan = self
            .session
            .ctx()
            .request_body
            .read_text_range(offset, max_bytes, self.limits.max_string_len, reserved)
            .await
            .map_err(map_body_error)?;
        match self.settle(reserved, scan) {
            BoundedTextResult::Complete(text) => Ok(text),
            BoundedTextResult::OutputLimitExceeded => Err(RuntimeError::StringTooLong {
                len: self.limits.max_string_len.saturating_add(1),
                limit: self.limits.max_string_len,
            }),
        }
    }

    async fn text_operation(&mut self, operation: BodyTextOperation) -> Result<BodyTextResult, RuntimeError> {
        let reserved = self.reserve()?;
        let scan = self
            .session
            .ctx()
            .request_body
            .text_operation(operation, reserved)
            .await
            .map_err(map_body_error)?;
        Ok(self.settle(reserved, scan))
    }

    pub(crate) async fn text_predicate(&mut self, operation: BodyTextOperation) -> Result<bool, RuntimeError> {
        Ok(matches!(
            self.text_operation(operation).await?,
            BodyTextResult::Bool(true)
        ))
    }

    pub(crate) async fn char_len(&mut self) -> Result<u64, RuntimeError> {
        match self.text_operation(BodyTextOperation::CharLen).await? {
            BodyTextResult::CharLen(len) => Ok(len),
            BodyTextResult::Bool(_) => Err(RuntimeError::Internal {
                message: "body CharLen returned Bool".to_string(),
            }),
        }
    }

    /// Construct the owned scanner after reserving, preserving constructor-error
    /// accounting for asynchronous JSON parsers and fallible regex scanners.
    pub(crate) async fn scan_with<S, F>(&mut self, factory: F) -> Result<S::Output, RuntimeError>
    where
        S: RawBodyScanner,
        F: Future<Output = Result<S, BodyOperationError>>,
    {
        let reserved = self.reserve()?;
        let scanner = factory.await.map_err(map_body_error)?;
        let scan = self
            .session
            .ctx()
            .request_body
            .scan_raw(scanner, reserved)
            .await
            .map_err(map_body_error)?;
        Ok(self.settle(reserved, scan))
    }

    pub(crate) async fn sha256(&mut self) -> Result<String, RuntimeError> {
        self.check_string_len(64)?;
        let reserved = self.reserve()?;
        let scan = self
            .session
            .ctx()
            .request_body
            .sha256_scan(reserved)
            .await
            .map_err(map_body_error)?;
        Ok(hex::encode(self.settle(reserved, scan)))
    }

    pub(crate) async fn md5(&mut self) -> Result<String, RuntimeError> {
        self.check_string_len(32)?;
        Ok(hex::encode(self.scan_with(async { Ok(Md5Scanner::new()) }).await?))
    }

    pub(crate) async fn hmac_sha256(&mut self, key: &[u8]) -> Result<String, RuntimeError> {
        self.check_string_len(64)?;
        let scanner = HmacSha256Scanner::new(key).map_err(map_body_error)?;
        Ok(hex::encode(self.scan_with(async { Ok(scanner) }).await?))
    }

    pub(crate) async fn clear(&mut self) -> Result<(), RuntimeError> {
        self.session.request_body_clear().await.map_err(map_body_error)
    }

    pub(crate) async fn append(&mut self, bytes: Bytes) -> Result<(), RuntimeError> {
        self.session.request_body_append(bytes).await.map_err(map_body_error)
    }

    pub(crate) async fn prepend(&mut self, bytes: Bytes) -> Result<(), RuntimeError> {
        self.session.request_body_prepend(bytes).await.map_err(map_body_error)
    }

    pub(crate) async fn remove(&mut self, range: Range<u64>) -> Result<(), RuntimeError> {
        self.session.request_body_remove(range).await.map_err(map_body_error)
    }

    pub(crate) async fn replace_literal(&mut self, from: Bytes, to: Bytes) -> Result<(), RuntimeError> {
        let reserved = self.reserve()?;
        let scan = self
            .session
            .request_body_replace_literal(from, to, reserved)
            .await
            .map_err(map_body_error)?;
        self.settle(reserved, scan);
        Ok(())
    }
}
