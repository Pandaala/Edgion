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

/// Complete occurrence counts for one selected operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GraphqlShapeReport {
    pub depth: usize,
    pub aliases: u32,
    pub root_fields: u32,
    pub fields: u32,
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
        self.inspect_shape_with_root_policy(limits, root_policy, false)
            .map(|_| ())
    }

    pub(crate) fn inspect_shape_with_root_policy(
        &mut self,
        limits: &GraphqlShapeLimits,
        root_policy: Option<&GraphqlRootFieldPolicy>,
        measure: bool,
    ) -> Result<GraphqlShapeReport, GraphqlError> {
        let mut traversal = Traversal {
            limits,
            root_policy,
            measure,
            fragments: &self.fragments,
            work: &mut self.work,
            report: GraphqlShapeReport::default(),
        };
        traversal.visit(self.operation.selection_set().ok_or(GraphqlError::Invalid)?, 1, 0)?;
        Ok(traversal.report)
    }
}

struct Traversal<'a> {
    limits: &'a GraphqlShapeLimits,
    root_policy: Option<&'a GraphqlRootFieldPolicy>,
    measure: bool,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    work: &'a mut Work,
    report: GraphqlShapeReport,
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
                    if !self.measure && self.limits.max_depth.is_some_and(|limit| depth > limit) {
                        return Err(GraphqlError::Invalid);
                    }
                    let name = field.name().ok_or(GraphqlError::Invalid)?.text();
                    if !self.limits.allow_introspection && matches!(name.as_ref(), "__schema" | "__type") {
                        return Err(GraphqlError::Invalid);
                    }
                    self.report.depth = self.report.depth.max(depth);
                    increment(
                        &mut self.report.fields,
                        self.limits.max_fields.filter(|_| !self.measure),
                    )?;
                    if field.alias().is_some() {
                        increment(
                            &mut self.report.aliases,
                            self.limits.max_aliases.filter(|_| !self.measure),
                        )?;
                    }
                    if depth == 1 {
                        if self.root_policy.is_some_and(|policy| policy.denies(name.as_ref())) {
                            return Err(GraphqlError::Forbidden);
                        }
                        increment(
                            &mut self.report.root_fields,
                            self.limits.max_root_fields.filter(|_| !self.measure),
                        )?;
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
