//! Independent shape protection and optional existing SDL cost ceiling.

use super::graphql_rate_limit::GraphqlCostRule;
use crate::graphql_cost::CompiledGraphqlCost;
use crate::graphql_request::{
    GraphqlError, GraphqlOperationType, GraphqlRootFieldPolicy, GraphqlShapeLimits, ParsedGraphqlRequest,
};
use crate::resources::edgion_plugins::PluginValidation;
use apollo_parser::{Lexer, TokenKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlGuardConfig {
    #[serde(default = "default_depth")]
    pub max_depth: Option<usize>,
    #[serde(default = "default_aliases")]
    pub max_aliases: Option<u32>,
    #[serde(default = "default_roots")]
    pub max_root_fields: Option<u32>,
    #[serde(default = "default_fields")]
    pub max_fields: Option<u32>,
    #[serde(default)]
    pub allow_introspection: bool,
    #[serde(default)]
    pub max_cost: Option<u32>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub cost_rules: Vec<GraphqlCostRule>,
    #[serde(default)]
    pub root_fields: Option<GraphqlRootFieldsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlRootFieldsConfig {
    #[serde(default)]
    pub query: Option<GraphqlRootFieldRuleConfig>,
    #[serde(default)]
    pub mutation: Option<GraphqlRootFieldRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlRootFieldRuleConfig {
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl GraphqlRootFieldRuleConfig {
    fn compile(&self) -> Result<GraphqlRootFieldPolicy, String> {
        fn names(source: &[String]) -> Result<HashSet<String>, String> {
            if source.len() > 256 || source.iter().any(|name| name.is_empty() || name.len() > 256) {
                return Err("root field lists require at most 256 names of 1..256 bytes".into());
            }
            let mut names = HashSet::new();
            for name in source {
                let valid = Lexer::new(name).with_limit(2).next().is_some_and(|token| {
                    token.is_ok_and(|token| token.kind() == TokenKind::Name && token.data() == name)
                });
                if !valid || !names.insert(name.clone()) {
                    return Err("root field lists require unique GraphQL names".into());
                }
            }
            Ok(names)
        }
        Ok(GraphqlRootFieldPolicy {
            allow: self.allow.as_deref().map(names).transpose()?,
            deny: names(&self.deny)?,
        })
    }
}

fn default_depth() -> Option<usize> {
    Some(20)
}
fn default_aliases() -> Option<u32> {
    Some(64)
}
fn default_roots() -> Option<u32> {
    Some(32)
}
fn default_fields() -> Option<u32> {
    Some(1024)
}

#[derive(Debug)]
pub struct CompiledGraphqlGuard {
    shape: GraphqlShapeLimits,
    cost: Option<CompiledGraphqlCost>,
    query_roots: Option<GraphqlRootFieldPolicy>,
    mutation_roots: Option<GraphqlRootFieldPolicy>,
}

impl CompiledGraphqlGuard {
    pub fn check(&self, mut parsed: ParsedGraphqlRequest) -> Result<(), GraphqlError> {
        let roots = match parsed.operation_type() {
            GraphqlOperationType::Query => self.query_roots.as_ref(),
            GraphqlOperationType::Mutation => self.mutation_roots.as_ref(),
        };
        parsed.validate_shape_with_root_policy(&self.shape, roots)?;
        if let Some(cost) = &self.cost {
            cost.estimate(parsed)?;
        }
        Ok(())
    }
}

impl GraphqlGuardConfig {
    pub fn compile(&self) -> Result<CompiledGraphqlGuard, String> {
        if self.max_depth.is_some_and(|n| !(1..=32).contains(&n)) {
            return Err("maxDepth must be null or 1..32".into());
        }
        if [self.max_aliases, self.max_root_fields, self.max_fields]
            .into_iter()
            .flatten()
            .any(|n| n > 4096)
        {
            return Err("shape count limits must be null or 0..4096".into());
        }
        let cost = if let Some(max_cost) = self.max_cost {
            if self.allow_introspection {
                return Err("allowIntrospection must be false when maxCost is configured".into());
            }
            let schema = self.schema.as_deref().ok_or("maxCost requires schema")?;
            Some(CompiledGraphqlCost::compile(
                schema,
                &self.cost_rules,
                max_cost,
                self.max_depth.unwrap_or(32),
            )?)
        } else {
            if self.schema.is_some() || !self.cost_rules.is_empty() {
                return Err("schema and costRules require maxCost".into());
            }
            None
        };
        Ok(CompiledGraphqlGuard {
            shape: GraphqlShapeLimits {
                max_depth: self.max_depth,
                max_aliases: self.max_aliases,
                max_root_fields: self.max_root_fields,
                max_fields: self.max_fields,
                allow_introspection: self.allow_introspection,
            },
            cost,
            query_roots: self
                .root_fields
                .as_ref()
                .and_then(|r| r.query.as_ref())
                .map(GraphqlRootFieldRuleConfig::compile)
                .transpose()?,
            mutation_roots: self
                .root_fields
                .as_ref()
                .and_then(|r| r.mutation.as_ref())
                .map(GraphqlRootFieldRuleConfig::compile)
                .transpose()?,
        })
    }
}

impl PluginValidation for GraphqlGuardConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/graphql_guard_test.rs"]
mod tests;
