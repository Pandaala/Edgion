//! Asserted GraphQL access-log tokens, shared by the related plugin suites.

pub const SHAPE: &str = "shape depth=";
pub const MEASURE: &str = "measure exceeded=";
pub const COST: &str = "cost=";
pub const COST_HEADER: &str = "X-Graphql-Query-Cost";

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/common/graphql_log_consts_test.rs"]
mod tests;
