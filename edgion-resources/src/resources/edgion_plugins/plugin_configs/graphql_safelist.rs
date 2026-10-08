//! Static whole-document authorization tied to the verified selected operation.

use crate::graphql_request::{GraphqlError, GraphqlOperationType, ParsedGraphqlRequest, MAX_ENVELOPE_BYTES};
use crate::resources::edgion_plugins::PluginValidation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlTrustedOperation {
    pub query: String,
    #[serde(default)]
    pub operation_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphqlSafelistConfig {
    pub operations: Vec<GraphqlTrustedOperation>,
}

#[derive(Debug, Default)]
struct TrustedNames {
    anonymous: bool,
    named: HashSet<String>,
}

#[derive(Debug)]
pub struct CompiledGraphqlSafelist {
    documents: HashMap<[u8; 32], HashMap<GraphqlOperationType, TrustedNames>>,
}

impl CompiledGraphqlSafelist {
    pub fn check(&self, mut parsed: ParsedGraphqlRequest) -> Result<(), GraphqlError> {
        let digest = parsed.document_fingerprint()?;
        let names = self
            .documents
            .get(&digest)
            .and_then(|operations| operations.get(&parsed.operation_type()))
            .ok_or(GraphqlError::Forbidden)?;
        let allowed = match parsed.operation_name() {
            Some(name) => names.named.contains(name),
            None => names.anonymous,
        };
        if allowed {
            Ok(())
        } else {
            Err(GraphqlError::Forbidden)
        }
    }
}

impl GraphqlSafelistConfig {
    pub fn compile(&self) -> Result<CompiledGraphqlSafelist, String> {
        if !(1..=128).contains(&self.operations.len()) {
            return Err("operations must contain 1..128 registrations".into());
        }
        let mut total = 0usize;
        for entry in &self.operations {
            if entry.query.len() > MAX_ENVELOPE_BYTES || entry.operation_name.as_ref().is_some_and(|n| n.len() > 256) {
                return Err("registered query or operationName exceeds its byte limit".into());
            }
            total = total
                .checked_add(entry.query.len())
                .ok_or("registered source size overflow")?;
            if total > 512 * 1024 {
                return Err("registered query sources exceed 512KiB".into());
            }
        }
        let mut compiled = CompiledGraphqlSafelist {
            documents: HashMap::new(),
        };
        for entry in &self.operations {
            let mut parsed = ParsedGraphqlRequest::parse_registered(&entry.query, entry.operation_name.as_deref())
                .map_err(|_| "invalid registered GraphQL document")?;
            let digest = parsed
                .document_fingerprint()
                .map_err(|_| "registered document exceeds fingerprint bounds")?;
            let names = compiled
                .documents
                .entry(digest)
                .or_default()
                .entry(parsed.operation_type())
                .or_default();
            let inserted = if let Some(name) = parsed.operation_name() {
                names.named.insert(name.to_owned())
            } else {
                let inserted = !names.anonymous;
                names.anonymous = true;
                inserted
            };
            if !inserted {
                return Err("duplicate trusted document and selected operation".into());
            }
        }
        Ok(compiled)
    }
}

impl PluginValidation for GraphqlSafelistConfig {
    fn validation_error(&self) -> Option<String> {
        self.compile().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/graphql_safelist_test.rs"]
mod tests;
