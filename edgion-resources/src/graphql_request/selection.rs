use super::{
    name, parse_document, GraphqlError, GraphqlOperationType, ParsedGraphqlRequest, Work, MAX_RECURSION, MAX_TOKENS,
};
use apollo_parser::cst::{self, CstNode};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(super) fn select(value: Value, get: bool) -> Result<ParsedGraphqlRequest, GraphqlError> {
    let envelope = super::envelope::envelope(&value)?;
    let document = parse_document(envelope.query)?;
    let mut operations = Vec::new();
    let mut fragments = HashMap::new();
    let mut operation_names = HashSet::new();
    let mut anonymous = false;
    let mut work = Work::default();
    for definition in document.definitions() {
        work.step()?;
        match definition {
            cst::Definition::OperationDefinition(op) => {
                if operations.len() >= MAX_TOKENS {
                    return Err(GraphqlError::Invalid);
                }
                directives(op.directives(), &mut work)?;
                if let Some(n) = op.name() {
                    if !operation_names.insert(n.text().to_string()) {
                        return Err(GraphqlError::Invalid);
                    }
                } else {
                    anonymous = true;
                }
                operations.push(op);
            }
            cst::Definition::FragmentDefinition(f) => {
                if fragments.len() >= MAX_TOKENS {
                    return Err(GraphqlError::Invalid);
                }
                directives(f.directives(), &mut work)?;
                let n = name(f.fragment_name().and_then(|n| n.name()))?;
                if fragments.insert(n, f).is_some() {
                    return Err(GraphqlError::Invalid);
                }
            }
            _ => return Err(GraphqlError::Invalid),
        }
    }
    if operations.is_empty() || (anonymous && operations.len() != 1) {
        return Err(GraphqlError::Invalid);
    }
    let operation = if let Some(n) = envelope.operation {
        operations
            .iter()
            .find(|o| o.name().is_some_and(|name| name.text() == n))
            .ok_or(GraphqlError::Invalid)?
    } else {
        if operations.len() != 1 {
            return Err(GraphqlError::Invalid);
        }
        &operations[0]
    };
    directives(operation.directives(), &mut work)?;
    let ty = operation.operation_type();
    if ty.as_ref().is_some_and(|o| o.subscription_token().is_some()) {
        return Err(GraphqlError::Invalid);
    }
    let kind = if ty.is_some_and(|o| o.mutation_token().is_some()) {
        if get {
            return Err(GraphqlError::MethodNotAllowed);
        }
        GraphqlOperationType::Mutation
    } else {
        GraphqlOperationType::Query
    };
    let mut vars = HashMap::new();
    if let Some(defs) = operation.variable_definitions() {
        for def in defs.variable_definitions() {
            work.step()?;
            if vars.len() >= MAX_TOKENS {
                return Err(GraphqlError::Invalid);
            }
            directives(def.directives(), &mut work)?;
            let n = name(def.variable().and_then(|v| v.name()))?;
            if vars.insert(n, def).is_some() {
                return Err(GraphqlError::Invalid);
            }
        }
    }
    // A graph-only walk detects cycles without expanding shared fragments.
    let mut states = HashMap::new();
    for n in fragments.keys() {
        graph_fragment(n, &fragments, &mut states, &mut work, 0)?;
    }
    for op in &operations {
        graph_set(
            &op.selection_set().ok_or(GraphqlError::Invalid)?,
            &fragments,
            &mut states,
            &mut work,
            0,
        )?;
    }
    let selected_name = operation.name().and_then(|n| n.syntax().first_token());
    let operation = operation.clone();
    Ok(ParsedGraphqlRequest {
        value,
        operation,
        selected_name,
        kind,
        fragments,
        vars,
        work,
    })
}

pub(crate) fn directives(source: Option<cst::Directives>, work: &mut Work) -> Result<(), GraphqlError> {
    if let Some(source) = source {
        for directive in source.directives() {
            work.step()?;
            let n = directive.name().ok_or(GraphqlError::Invalid)?.text();
            if !matches!(n.as_str(), "skip" | "include") {
                return Err(GraphqlError::Invalid);
            }
            let args = directive.arguments().ok_or(GraphqlError::Invalid)?;
            let mut count = 0;
            for arg in args.arguments() {
                work.step()?;
                count += 1;
                if arg.name().ok_or(GraphqlError::Invalid)?.text() != "if"
                    || !matches!(arg.value(), Some(cst::Value::BooleanValue(_) | cst::Value::Variable(_)))
                {
                    return Err(GraphqlError::Invalid);
                }
            }
            if count != 1 {
                return Err(GraphqlError::Invalid);
            }
        }
    }
    Ok(())
}
fn graph_fragment<'a>(
    n: &str,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    states: &mut HashMap<&'a str, u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), GraphqlError> {
    work.step()?;
    if depth >= MAX_RECURSION {
        return Err(GraphqlError::Invalid);
    }
    match states.get(n) {
        Some(1) => return Err(GraphqlError::Invalid),
        Some(2) => return Ok(()),
        _ => {}
    }
    let (key, fragment) = fragments.get_key_value(n).ok_or(GraphqlError::Invalid)?;
    states.insert(key.as_str(), 1);
    graph_set(
        &fragment.selection_set().ok_or(GraphqlError::Invalid)?,
        fragments,
        states,
        work,
        depth + 1,
    )?;
    states.insert(key.as_str(), 2);
    Ok(())
}
fn graph_set<'a>(
    set: &cst::SelectionSet,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    states: &mut HashMap<&'a str, u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), GraphqlError> {
    if depth >= MAX_RECURSION {
        return Err(GraphqlError::Invalid);
    }
    for selection in set.selections() {
        work.step()?;
        match selection {
            cst::Selection::FragmentSpread(s) => {
                directives(s.directives(), work)?;
                graph_fragment(
                    &name(s.fragment_name().and_then(|n| n.name()))?,
                    fragments,
                    states,
                    work,
                    depth + 1,
                )?;
            }
            cst::Selection::InlineFragment(f) => {
                directives(f.directives(), work)?;
                graph_set(
                    &f.selection_set().ok_or(GraphqlError::Invalid)?,
                    fragments,
                    states,
                    work,
                    depth + 1,
                )?;
            }
            cst::Selection::Field(f) => {
                directives(f.directives(), work)?;
                if let Some(args) = f.arguments() {
                    let mut names = HashSet::new();
                    for arg in args.arguments() {
                        work.step()?;
                        if names.len() >= MAX_TOKENS || !names.insert(name(arg.name())?) {
                            return Err(GraphqlError::Invalid);
                        }
                    }
                }
                if let Some(set) = f.selection_set() {
                    graph_set(&set, fragments, states, work, depth + 1)?;
                }
            }
        }
    }
    Ok(())
}
