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
    root_fields: Vec<(String, String)>,
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_cost/mod_test.rs"]
mod tests;

/// Maximum independent root slots; the mandatory total occupies the other slot.
pub const MAX_ROOT_QUOTAS: usize = 15;

/// Completed cost report with no request-owned AST, variables or heap storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphqlQuotaCosts {
    pub total: u32,
    pub roots: [u32; MAX_ROOT_QUOTAS],
}

impl CompiledGraphqlCost {
    /// Bind actual SDL query/mutation root coordinates at cold construction.
    pub fn with_root_fields<'a>(mut self, fields: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        for coordinate in fields {
            if self.root_fields.len() >= MAX_ROOT_QUOTAS {
                return Err("Too many GraphQL root quotas".into());
            }
            let (ty, field) = coordinate.split_once('.').ok_or("Invalid GraphQL root coordinate")?;
            if (ty != self.schema.query && self.schema.mutation.as_deref() != Some(ty))
                || field.starts_with("__")
                || !self.schema.types[ty].fields.contains_key(field)
            {
                return Err("Quota field must be an existing query/mutation root field".into());
            }
            self.root_fields.push((ty.into(), field.into()));
        }
        Ok(self)
    }
}
