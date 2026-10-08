//! Bounded, runtime-independent GraphQL cost estimation.

mod request;
mod schema;

#[cfg(test)]
use crate::graphql_request::MAX_ENVELOPE_BYTES;
use crate::graphql_request::{name, parse_document, GraphqlError, Work, MAX_RECURSION};
use crate::resources::edgion_plugins::plugin_configs::GraphqlCostRule;
use std::collections::HashMap;

#[derive(Debug)]
pub struct CompiledGraphqlCost {
    schema: schema::Schema,
    rules: HashMap<String, GraphqlCostRule>,
    max_cost: u32,
    max_depth: usize,
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_cost/mod_test.rs"]
mod tests;
