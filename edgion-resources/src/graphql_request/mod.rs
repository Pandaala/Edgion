//! Canonical bounded GraphQL parsing and verified operation selection.

mod envelope;
mod selection;
pub(crate) use selection::directives;

use apollo_parser::{cst, Lexer, Parser, TokenKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

pub const MAX_ENVELOPE_BYTES: usize = 65_536;
pub(crate) const MAX_TOKENS: usize = 4096;
const MAX_WORK: usize = 32_768;
pub(crate) const MAX_RECURSION: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphqlError {
    Invalid,
    TooLarge,
    MethodNotAllowed,
}

impl fmt::Display for GraphqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid or unsupported GraphQL request",
            Self::TooLarge => "GraphQL envelope exceeds its limit",
            Self::MethodNotAllowed => "GraphQL operation does not support this method",
        })
    }
}
impl std::error::Error for GraphqlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GraphqlOperationType {
    Query,
    Mutation,
}

/// One bounded JSON value and CST, owned only during request evaluation.
#[derive(Debug)]
pub struct ParsedGraphqlRequest {
    pub(crate) value: Value,
    pub(crate) operation: cst::OperationDefinition,
    selected_name: Option<apollo_parser::SyntaxToken>,
    kind: GraphqlOperationType,
    pub(crate) fragments: HashMap<String, cst::FragmentDefinition>,
    pub(crate) vars: HashMap<String, cst::VariableDefinition>,
    pub(crate) work: Work,
}
impl ParsedGraphqlRequest {
    pub fn parse_post(bytes: &[u8], limit: usize) -> Result<Self, GraphqlError> {
        selection::select(envelope::post(bytes, limit)?, false)
    }
    pub fn parse_get(query: &str, limit: usize) -> Result<Self, GraphqlError> {
        selection::select(envelope::get(query, limit)?, true)
    }
    pub fn operation_type(&self) -> GraphqlOperationType {
        self.kind
    }
    pub fn operation_name(&self) -> Option<&str> {
        self.selected_name.as_ref().map(|n| n.text())
    }
}

#[derive(Debug, Default)]
pub(crate) struct Work(usize);
impl Work {
    pub(crate) fn step(&mut self) -> Result<(), GraphqlError> {
        self.0 += 1;
        if self.0 > MAX_WORK {
            Err(GraphqlError::Invalid)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn parse_document(source: &str) -> Result<cst::Document, GraphqlError> {
    if source.len() > MAX_ENVELOPE_BYTES {
        return Err(GraphqlError::TooLarge);
    }
    for token in Lexer::new(source).with_limit(MAX_TOKENS) {
        let token = token.map_err(|_| GraphqlError::Invalid)?;
        if !matches!(
            token.kind(),
            TokenKind::Whitespace | TokenKind::Comment | TokenKind::Comma | TokenKind::Eof
        ) && token.data().len() > 256
        {
            return Err(GraphqlError::Invalid);
        }
    }
    let parsed = Parser::new(source)
        .recursion_limit(MAX_RECURSION)
        .token_limit(MAX_TOKENS)
        .parse();
    if parsed.errors().next().is_some() {
        return Err(GraphqlError::Invalid);
    }
    Ok(parsed.document())
}

pub(crate) fn name(name: Option<cst::Name>) -> Result<String, GraphqlError> {
    name.map(|n| n.text().to_string()).ok_or(GraphqlError::Invalid)
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_request/mod_test.rs"]
mod tests;
