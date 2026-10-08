//! Conservative selected-operation shape checks without an expanded AST.

use super::{GraphqlError, ParsedGraphqlRequest, Work, MAX_RECURSION};
use apollo_parser::cst;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct GraphqlRootFieldPolicy {
    pub(crate) allow: Option<HashSet<String>>,
    pub(crate) deny: HashSet<String>,
}

impl GraphqlRootFieldPolicy {
    fn denies(&self, name: &str) -> bool {
        self.deny.contains(name) || self.allow.as_ref().is_some_and(|allow| !allow.contains(name))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GraphqlShapeLimits {
    pub max_depth: Option<usize>,
    pub max_aliases: Option<u32>,
    pub max_root_fields: Option<u32>,
    pub max_fields: Option<u32>,
    pub allow_introspection: bool,
}

impl ParsedGraphqlRequest {
    pub fn validate_shape(&mut self, limits: &GraphqlShapeLimits) -> Result<(), GraphqlError> {
        self.validate_shape_with_root_policy(limits, None)
    }

    pub(crate) fn validate_shape_with_root_policy(
        &mut self,
        limits: &GraphqlShapeLimits,
        root_policy: Option<&GraphqlRootFieldPolicy>,
    ) -> Result<(), GraphqlError> {
        let mut traversal = Traversal {
            limits,
            root_policy,
            fragments: &self.fragments,
            work: &mut self.work,
            aliases: 0,
            roots: 0,
            fields: 0,
        };
        traversal.visit(self.operation.selection_set().ok_or(GraphqlError::Invalid)?, 1, 0)
    }
}

struct Traversal<'a> {
    limits: &'a GraphqlShapeLimits,
    root_policy: Option<&'a GraphqlRootFieldPolicy>,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    work: &'a mut Work,
    aliases: u32,
    roots: u32,
    fields: u32,
}

fn increment(count: &mut u32, limit: Option<u32>) -> Result<(), GraphqlError> {
    *count = count.checked_add(1).ok_or(GraphqlError::Invalid)?;
    if limit.is_some_and(|limit| *count > limit) {
        return Err(GraphqlError::Invalid);
    }
    Ok(())
}

impl Traversal<'_> {
    fn visit(&mut self, set: cst::SelectionSet, depth: usize, recursion: usize) -> Result<(), GraphqlError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlError::Invalid);
        }
        for selection in set.selections() {
            self.work.step()?;
            match selection {
                cst::Selection::Field(field) => {
                    if self.limits.max_depth.is_some_and(|limit| depth > limit) {
                        return Err(GraphqlError::Invalid);
                    }
                    let name = field.name().ok_or(GraphqlError::Invalid)?.text();
                    if !self.limits.allow_introspection && matches!(name.as_ref(), "__schema" | "__type") {
                        return Err(GraphqlError::Invalid);
                    }
                    increment(&mut self.fields, self.limits.max_fields)?;
                    if field.alias().is_some() {
                        increment(&mut self.aliases, self.limits.max_aliases)?;
                    }
                    if depth == 1 {
                        if self.root_policy.is_some_and(|policy| policy.denies(name.as_ref())) {
                            return Err(GraphqlError::Forbidden);
                        }
                        increment(&mut self.roots, self.limits.max_root_fields)?;
                    }
                    if let Some(children) = field.selection_set() {
                        self.visit(children, depth + 1, recursion + 1)?;
                    }
                }
                cst::Selection::InlineFragment(fragment) => {
                    self.visit(
                        fragment.selection_set().ok_or(GraphqlError::Invalid)?,
                        depth,
                        recursion + 1,
                    )?;
                }
                cst::Selection::FragmentSpread(spread) => {
                    let name = spread
                        .fragment_name()
                        .and_then(|name| name.name())
                        .ok_or(GraphqlError::Invalid)?
                        .text();
                    let fragment = self.fragments.get(name.as_ref()).ok_or(GraphqlError::Invalid)?;
                    self.visit(
                        fragment.selection_set().ok_or(GraphqlError::Invalid)?,
                        depth,
                        recursion + 1,
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_request/guard_test.rs"]
mod tests;
