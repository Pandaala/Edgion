use super::{GraphqlError, ParsedGraphqlRequest, MAX_ENVELOPE_BYTES, MAX_TOKENS};
use apollo_parser::{Lexer, TokenKind};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

impl ParsedGraphqlRequest {
    /// Cold registration uses the same document selection without a JSON wire copy.
    pub(crate) fn parse_registered(query: &str, operation_name: Option<&str>) -> Result<Self, GraphqlError> {
        if query.len() > MAX_ENVELOPE_BYTES || operation_name.is_some_and(|name| name.len() > 256) {
            return Err(GraphqlError::TooLarge);
        }
        let mut value = Map::new();
        value.insert("query".into(), Value::String(query.into()));
        if let Some(name) = operation_name {
            value.insert("operationName".into(), Value::String(name.into()));
        }
        super::selection::select(Value::Object(value), false)
    }

    /// Hash the complete validated document, carrying the frontend work ledger.
    pub(crate) fn document_fingerprint(&mut self) -> Result<[u8; 32], GraphqlError> {
        let source = self
            .value
            .get("query")
            .and_then(Value::as_str)
            .ok_or(GraphqlError::Invalid)?;
        let mut digest = Sha256::new();
        for token in Lexer::new(source).with_limit(MAX_TOKENS) {
            self.work.step()?;
            let token = token.map_err(|_| GraphqlError::Invalid)?;
            if matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::Comment | TokenKind::Comma | TokenKind::Eof
            ) {
                continue;
            }
            let bytes = token.data().as_bytes();
            digest.update((bytes.len() as u32).to_be_bytes());
            digest.update(bytes);
        }
        Ok(digest.finalize().into())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_request/safelist_test.rs"]
mod tests;
