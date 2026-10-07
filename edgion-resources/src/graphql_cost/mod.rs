//! Bounded, runtime-independent GraphQL cost estimation.

mod request;
mod schema;

use apollo_parser::{cst, Lexer, Parser, TokenKind};
use std::collections::HashMap;
use std::fmt;

use crate::resources::edgion_plugins::plugin_configs::GraphqlCostRule;

pub const MAX_ENVELOPE_BYTES: usize = 65_536;
const MAX_TOKENS: usize = 4096;
const MAX_WORK: usize = 32_768;
const MAX_RECURSION: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphqlCostError {
    Invalid,
    TooLarge,
    MethodNotAllowed,
}

impl fmt::Display for GraphqlCostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid or unsupported GraphQL request",
            Self::TooLarge => "GraphQL envelope exceeds its limit",
            Self::MethodNotAllowed => "GraphQL operation does not support this method",
        })
    }
}
impl std::error::Error for GraphqlCostError {}

#[derive(Debug)]
pub struct CompiledGraphqlCost {
    schema: schema::Schema,
    rules: HashMap<String, GraphqlCostRule>,
    max_cost: u32,
    max_depth: usize,
}

#[derive(Default)]
struct Work(usize);
impl Work {
    fn step(&mut self) -> Result<(), GraphqlCostError> {
        self.0 += 1;
        if self.0 > MAX_WORK {
            Err(GraphqlCostError::Invalid)
        } else {
            Ok(())
        }
    }
}

fn parse_document(source: &str) -> Result<cst::Document, GraphqlCostError> {
    if source.len() > MAX_ENVELOPE_BYTES {
        return Err(GraphqlCostError::TooLarge);
    }
    for token in Lexer::new(source).with_limit(MAX_TOKENS) {
        let token = token.map_err(|_| GraphqlCostError::Invalid)?;
        if !matches!(
            token.kind(),
            TokenKind::Whitespace | TokenKind::Comment | TokenKind::Comma | TokenKind::Eof
        ) && token.data().len() > 256
        {
            return Err(GraphqlCostError::Invalid);
        }
    }
    let parsed = Parser::new(source)
        .recursion_limit(MAX_RECURSION)
        .token_limit(MAX_TOKENS)
        .parse();
    if parsed.errors().next().is_some() {
        return Err(GraphqlCostError::Invalid);
    }
    Ok(parsed.document())
}

fn name(name: Option<cst::Name>) -> Result<String, GraphqlCostError> {
    name.map(|n| n.text().to_string()).ok_or(GraphqlCostError::Invalid)
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_cost/mod_test.rs"]
mod tests;
