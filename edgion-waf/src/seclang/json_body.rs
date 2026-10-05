//! Metadata-only JSON request-body completion and canonical publication accounting.

use std::fmt;

use thiserror::Error;

use super::body::BodyAccounting;
use super::{
    BodyError, BodyLimits, BodyProcessorSelection, JsonDocument, JsonDocumentError, JsonDocumentEvent,
    JsonDocumentLimits, JsonDocumentSinkError, JsonDocumentUsage, JsonPublicationSummary, JsonStreamError,
    JsonStreamInput, JsonStreamLimits, JsonStreamUsage,
};

#[derive(Debug, Clone, Copy)]
pub struct JsonBodyConfig {
    pub lexical: JsonStreamLimits,
    pub document: JsonDocumentLimits,
    pub body: BodyLimits,
    pub head_args_combined_size: usize,
    /// None omits raw publication on success and includes it on malformed fallback.
    pub retain_raw_body: Option<bool>,
    pub detection_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonBodySummary {
    pub publication: JsonPublicationSummary,
    /// The original malformed-body error when detection-only fallback succeeds.
    pub body_error: Option<BodyError>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum JsonBodyError {
    #[error("JSON body source exceeds the independent limit of {limit} bytes")]
    SourceByteLimit { limit: usize },
    #[error("JSON body source accounting arithmetic overflowed")]
    Arithmetic,
    #[error("JSON body document failed: {0}")]
    Document(JsonDocumentError),
    #[error(transparent)]
    Body(#[from] BodyError),
    #[error("JSON body was abandoned")]
    Abandoned,
    #[error("JSON body is already complete")]
    AlreadyComplete,
}

#[derive(Debug, Clone, Copy)]
enum Terminal {
    Active,
    Complete(JsonBodySummary),
    Failed(JsonBodyError),
}

/// A bounded metadata adapter, not a body cache or a rule executor.
///
/// Each Data chunk is admitted in full against `lexical.max_source_bytes`
/// before any callback or parsing. That independent source limit takes precedence
/// over errors inside the chunk and remains enforced while draining malformed
/// input. Canonical BodyLimits are deferred until Complete, when final length is
/// known. Syntax errors discard provisional metadata and drain lengths only;
/// resource, allocation and callback errors are immediate and never fall back.
///
/// Callback results remain provisional until completion. Caller-owned callback
/// state needs independent limits and must not retain payloads. Successful valid
/// documents retain only their bounded event arena. Empty bodies and malformed
/// detection-only fallback retain no document. Raw publication is logical only:
/// raw bytes and reconstructed scalar payloads are never retained here.
pub struct JsonBodyAdapter<S: Copy> {
    config: JsonBodyConfig,
    document: Option<JsonDocument<S>>,
    source_bytes: usize,
    malformed: bool,
    terminal: Terminal,
}

impl<S: Copy> fmt::Debug for JsonBodyAdapter<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonBodyAdapter")
            .field("source_bytes", &self.source_bytes)
            .field("malformed", &self.malformed)
            .field("terminal", &self.terminal)
            .field("usage", &self.usage())
            .finish()
    }
}

impl<S: Copy> JsonBodyAdapter<S> {
    pub fn new(config: JsonBodyConfig) -> Self {
        Self {
            document: Some(JsonDocument::new_for_body(config.lexical, config.document)),
            config,
            source_bytes: 0,
            malformed: false,
            terminal: Terminal::Active,
        }
    }

    /// Bytes in admitted chunks, including their remainder after a syntax error.
    /// A chunk rejected by independent source admission is not counted.
    pub const fn source_bytes(&self) -> usize {
        self.source_bytes
    }

    pub fn usage(&self) -> Option<JsonDocumentUsage> {
        self.document.as_ref().map(JsonDocument::usage)
    }

    pub fn lexical_usage(&self) -> Option<JsonStreamUsage> {
        self.document.as_ref().map(JsonDocument::lexical_usage)
    }

    pub fn summary(&self) -> Option<JsonBodySummary> {
        match self.terminal {
            Terminal::Complete(summary) => Some(summary),
            _ => None,
        }
    }

    pub fn events(&self) -> Option<impl Iterator<Item = JsonDocumentEvent<S>> + '_> {
        if !matches!(self.terminal, Terminal::Complete(_)) {
            return None;
        }
        self.document.as_ref()?.events()
    }

    pub fn feed<F>(&mut self, input: JsonStreamInput<'_>, mut sink: F) -> Result<(), JsonBodyError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        match self.terminal {
            Terminal::Complete(_) => return Err(JsonBodyError::AlreadyComplete),
            Terminal::Failed(error) => return Err(error),
            Terminal::Active => {}
        }
        let result = self.feed_active(input, &mut sink);
        if let Err(error) = result {
            self.document = None;
            self.terminal = Terminal::Failed(error);
        }
        result
    }

    fn feed_active<F>(&mut self, input: JsonStreamInput<'_>, sink: &mut F) -> Result<(), JsonBodyError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        match input {
            JsonStreamInput::Abandoned => return Err(JsonBodyError::Abandoned),
            JsonStreamInput::Data(bytes) => {
                let next = self
                    .source_bytes
                    .checked_add(bytes.len())
                    .ok_or(JsonBodyError::Arithmetic)?;
                if next > self.config.lexical.max_source_bytes {
                    return Err(JsonBodyError::SourceByteLimit {
                        limit: self.config.lexical.max_source_bytes,
                    });
                }
                self.source_bytes = next;
                self.feed_document(input, sink)?;
            }
            JsonStreamInput::Complete => {
                if self.source_bytes == 0 {
                    self.document = None;
                } else {
                    self.feed_document(input, sink)?;
                }
                let publication = self.complete_publication()?;
                self.terminal = Terminal::Complete(publication);
            }
        }
        Ok(())
    }

    fn feed_document<F>(&mut self, input: JsonStreamInput<'_>, sink: &mut F) -> Result<(), JsonBodyError>
    where
        F: FnMut(&str, &[u8]) -> Result<S, JsonDocumentSinkError>,
    {
        let Some(document) = self.document.as_mut() else {
            return Ok(());
        };
        match document.feed(input, sink) {
            Ok(()) => Ok(()),
            Err(JsonDocumentError::Lexical(
                JsonStreamError::Syntax
                | JsonStreamError::Utf8
                | JsonStreamError::Escape
                | JsonStreamError::Number
                | JsonStreamError::Incomplete,
            )) => {
                self.malformed = true;
                self.document = None;
                Ok(())
            }
            Err(error) => Err(JsonBodyError::Document(error)),
        }
    }

    fn complete_publication(&self) -> Result<JsonBodySummary, JsonBodyError> {
        // Prelude failure must precede malformed syntax and must never use fallback.
        let mut accounting = BodyAccounting::json_prelude(
            self.source_bytes,
            self.config.body,
            self.config.retain_raw_body.unwrap_or(false),
        )?;
        let body_error = if self.malformed {
            let error = BodyError::Malformed {
                processor: BodyProcessorSelection::Json,
            };
            if !self.config.detection_only {
                return Err(JsonBodyError::Body(error));
            }
            accounting = BodyAccounting::json_prelude(
                self.source_bytes,
                self.config.body,
                self.config.retain_raw_body.unwrap_or(true),
            )?;
            Some(error)
        } else {
            if let Some(document) = &self.document {
                let events = document
                    .events()
                    .ok_or(JsonBodyError::Document(JsonDocumentError::State))?;
                for event in events {
                    accounting.replay_json(event)?;
                }
            }
            None
        };
        Ok(JsonBodySummary {
            publication: accounting.finish_json(self.config.head_args_combined_size)?,
            body_error,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/json_body_test.rs"]
mod tests;
