use super::schema::{DefaultValue, TypeRef};
use super::{CompiledGraphqlCost, GraphqlError, GraphqlQuotaCosts, MAX_RECURSION, MAX_ROOT_QUOTAS};
use crate::graphql_request::{directives, name, GraphqlOperationType, ParsedGraphqlRequest, Work, MAX_TOKENS};
use apollo_parser::cst;
use serde_json::{Map, Value};
use std::collections::HashMap;

impl CompiledGraphqlCost {
    pub fn estimate_post(&self, bytes: &[u8], limit: usize) -> Result<u32, GraphqlError> {
        self.estimate(ParsedGraphqlRequest::parse_post(bytes, limit)?)
    }
    pub fn estimate_get(&self, query: &str, limit: usize) -> Result<u32, GraphqlError> {
        self.estimate(ParsedGraphqlRequest::parse_get(query, limit)?)
    }
    pub fn estimate(&self, parsed: ParsedGraphqlRequest) -> Result<u32, GraphqlError> {
        self.estimate_quotas(parsed).map(|costs| costs.total)
    }
    pub fn estimate_quotas(&self, parsed: ParsedGraphqlRequest) -> Result<GraphqlQuotaCosts, GraphqlError> {
        let root = match parsed.operation_type() {
            GraphqlOperationType::Query => &self.schema.query,
            GraphqlOperationType::Mutation => self.schema.mutation.as_ref().ok_or(GraphqlError::Invalid)?,
        };
        let operation = &parsed.operation;
        let fragments = parsed.fragments;
        let vars = parsed.vars;
        let work = parsed.work;
        let mut evaluation = Evaluation {
            model: self,
            fragments: &fragments,
            vars: &vars,
            values: parsed.value.get("variables").and_then(Value::as_object),
            work,
            root_costs: [0; MAX_ROOT_QUOTAS],
        };
        evaluation.validate_set(&operation.selection_set().ok_or(GraphqlError::Invalid)?, root, 0)?;
        for f in fragments.values() {
            let ty = name(f.type_condition().and_then(|t| t.named_type()).and_then(|t| t.name()))?;
            evaluation.validate_set(&f.selection_set().ok_or(GraphqlError::Invalid)?, &ty, 0)?;
        }
        let cost = evaluation.set(
            &operation.selection_set().ok_or(GraphqlError::Invalid)?,
            root,
            root,
            1,
            0,
        )?;
        Ok(GraphqlQuotaCosts {
            total: evaluation.add(1, cost)?,
            roots: evaluation.root_costs,
        })
    }
}
struct Evaluation<'a> {
    model: &'a CompiledGraphqlCost,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    vars: &'a HashMap<String, cst::VariableDefinition>,
    values: Option<&'a Map<String, Value>>,
    work: Work,
    root_costs: [u32; MAX_ROOT_QUOTAS],
}
impl Evaluation<'_> {
    // Validate static selection shapes without following or expanding spreads.
    // Fragment bodies are checked once each in their declared context.
    fn validate_set(&mut self, set: &cst::SelectionSet, ty: &str, recursion: usize) -> Result<(), GraphqlError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlError::Invalid);
        }
        let source = self
            .model
            .schema
            .types
            .get(ty)
            .filter(|t| t.composite())
            .ok_or(GraphqlError::Invalid)?;
        for selection in set.selections() {
            self.work.step()?;
            match selection {
                cst::Selection::Field(f) => {
                    let n = name(f.name())?;
                    if n == "__typename" {
                        if f.arguments().is_some() || f.selection_set().is_some() {
                            return Err(GraphqlError::Invalid);
                        }
                        continue;
                    }
                    if n.starts_with("__") {
                        return Err(GraphqlError::Invalid);
                    }
                    let field = source.fields.get(&n).ok_or(GraphqlError::Invalid)?;
                    if let Some(args) = f.arguments() {
                        for arg in args.arguments() {
                            self.work.step()?;
                            if !field.args.contains_key(&name(arg.name())?) {
                                return Err(GraphqlError::Invalid);
                            }
                        }
                    }
                    let returned = &self.model.schema.types[&field.ty.name];
                    match (returned.composite(), f.selection_set()) {
                        (true, Some(set)) => self.validate_set(&set, &field.ty.name, recursion + 1)?,
                        (false, None) => {}
                        _ => return Err(GraphqlError::Invalid),
                    }
                }
                cst::Selection::InlineFragment(f) => {
                    let target = match f.type_condition() {
                        Some(t) => name(t.named_type().and_then(|t| t.name()))?,
                        None => ty.to_string(),
                    };
                    self.condition(ty, "", &target)?;
                    self.validate_set(&f.selection_set().ok_or(GraphqlError::Invalid)?, &target, recursion + 1)?;
                }
                cst::Selection::FragmentSpread(s) => {
                    let f = &self.fragments[&name(s.fragment_name().and_then(|n| n.name()))?];
                    let target = name(f.type_condition().and_then(|t| t.named_type()).and_then(|t| t.name()))?;
                    self.condition(ty, "", &target)?;
                }
            }
        }
        Ok(())
    }
    fn add(&self, a: u32, b: u32) -> Result<u32, GraphqlError> {
        a.checked_add(b)
            .filter(|n| *n <= self.model.max_cost)
            .ok_or(GraphqlError::Invalid)
    }
    fn set(
        &mut self,
        set: &cst::SelectionSet,
        static_ty: &str,
        runtime_ty: &str,
        depth: usize,
        recursion: usize,
    ) -> Result<u32, GraphqlError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlError::Invalid);
        }
        let t = self
            .model
            .schema
            .types
            .get(runtime_ty)
            .filter(|t| t.composite())
            .ok_or(GraphqlError::Invalid)?;
        if t.possible.is_empty() {
            return Err(GraphqlError::Invalid);
        }
        let mut maximum = 0;
        for object in &t.possible {
            self.work.step()?;
            maximum = maximum.max(self.object_set(set, static_ty, object, depth, recursion + 1)?);
        }
        Ok(maximum)
    }
    fn condition(&mut self, static_ty: &str, object: &str, condition: &str) -> Result<bool, GraphqlError> {
        let source = &self.model.schema.types[static_ty];
        let target = self
            .model
            .schema
            .types
            .get(condition)
            .filter(|t| t.composite())
            .ok_or(GraphqlError::Invalid)?;
        let mut overlap = false;
        let mut applies = false;
        for possible in &target.possible {
            self.work.step()?;
            if possible == object {
                applies = true;
            }
            for candidate in &source.possible {
                self.work.step()?;
                if candidate == possible {
                    overlap = true;
                    break;
                }
            }
        }
        if !overlap {
            return Err(GraphqlError::Invalid);
        }
        Ok(applies)
    }
    fn object_set(
        &mut self,
        set: &cst::SelectionSet,
        static_ty: &str,
        object: &str,
        depth: usize,
        recursion: usize,
    ) -> Result<u32, GraphqlError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlError::Invalid);
        }
        let mut total = 0;
        for selection in set.selections() {
            self.work.step()?;
            let cost = match selection {
                cst::Selection::Field(f) => self.field(&f, static_ty, object, depth, recursion + 1)?,
                cst::Selection::InlineFragment(f) => {
                    directives(f.directives(), &mut self.work)?;
                    let ty = match f.type_condition() {
                        Some(t) => name(t.named_type().and_then(|t| t.name()))?,
                        None => static_ty.to_string(),
                    };
                    if self.condition(static_ty, object, &ty)? {
                        self.object_set(
                            &f.selection_set().ok_or(GraphqlError::Invalid)?,
                            &ty,
                            object,
                            depth,
                            recursion + 1,
                        )?
                    } else {
                        0
                    }
                }
                cst::Selection::FragmentSpread(s) => {
                    directives(s.directives(), &mut self.work)?;
                    let n = name(s.fragment_name().and_then(|n| n.name()))?;
                    let f = self.fragments.get(&n).ok_or(GraphqlError::Invalid)?;
                    let ty = name(f.type_condition().and_then(|t| t.named_type()).and_then(|t| t.name()))?;
                    if self.condition(static_ty, object, &ty)? {
                        self.object_set(
                            &f.selection_set().ok_or(GraphqlError::Invalid)?,
                            &ty,
                            object,
                            depth,
                            recursion + 1,
                        )?
                    } else {
                        0
                    }
                }
            };
            total = self.add(total, cost)?;
        }
        Ok(total)
    }
    fn field(
        &mut self,
        field: &cst::Field,
        static_ty: &str,
        object: &str,
        depth: usize,
        recursion: usize,
    ) -> Result<u32, GraphqlError> {
        if depth > self.model.max_depth {
            return Err(GraphqlError::Invalid);
        }
        directives(field.directives(), &mut self.work)?;
        let n = name(field.name())?;
        if n == "__typename" {
            if field.selection_set().is_some() || field.arguments().is_some() {
                return Err(GraphqlError::Invalid);
            }
            return Ok(1);
        }
        if n.starts_with("__") || !self.model.schema.types[static_ty].fields.contains_key(&n) {
            return Err(GraphqlError::Invalid);
        }
        let declared = &self.model.schema.types[static_ty].fields[&n];
        let actual = self.model.schema.types[object]
            .fields
            .get(&n)
            .ok_or(GraphqlError::Invalid)?;
        let mut args = HashMap::new();
        if let Some(arguments) = field.arguments() {
            for arg in arguments.arguments() {
                self.work.step()?;
                if args.len() >= MAX_TOKENS {
                    return Err(GraphqlError::Invalid);
                }
                let key = name(arg.name())?;
                if !actual.args.contains_key(&key)
                    || args.insert(key, arg.value().ok_or(GraphqlError::Invalid)?).is_some()
                {
                    return Err(GraphqlError::Invalid);
                }
            }
        }
        let return_type = &self.model.schema.types[&actual.ty.name];
        let children = match (return_type.composite(), field.selection_set()) {
            (true, Some(set)) => self.set(&set, &declared.ty.name, &actual.ty.name, depth + 1, recursion)?,
            (false, None) => 0,
            _ => return Err(GraphqlError::Invalid),
        };
        let concrete = &self.model.schema.types[object];
        let mut cost = None;
        for candidate in std::iter::once(Some(actual)).chain(
            concrete
                .interfaces
                .iter()
                .map(|interface| self.model.schema.types[interface].fields.get(&n)),
        ) {
            // Missing interface fields and unconfigured candidates still consume work.
            self.work.step()?;
            if let Some(index) = candidate.and_then(|field| field.rule_index) {
                let rule = self
                    .model
                    .rules
                    .get(usize::from(index.get()) - 1)
                    .ok_or(GraphqlError::Invalid)?;
                let mut factor = 1u32;
                for argument in &rule.multipliers {
                    self.work.step()?;
                    let supplied = args.get(argument).map(|v| self.multiplier(v)).transpose()?.flatten();
                    let value = match supplied {
                        Some(value) => value,
                        None => match actual.args[argument].default {
                            DefaultValue::Int(i) => positive_int(i)?,
                            DefaultValue::Missing => rule.default_multiplier.max(1),
                            _ => return Err(GraphqlError::Invalid),
                        },
                    };
                    factor = factor.checked_mul(value).ok_or(GraphqlError::Invalid)?;
                }
                let candidate = self.add(rule.weight, factor.checked_mul(children).ok_or(GraphqlError::Invalid)?)?;
                cost = Some(cost.unwrap_or(0u32).max(candidate));
            }
        }
        let charge = cost.map(Ok).unwrap_or_else(|| self.add(1, children))?;
        if depth == 1 {
            for (index, (root_type, field_name)) in self.model.root_fields.iter().enumerate() {
                self.work.step()?;
                if root_type == object && field_name == &n {
                    self.root_costs[index] = self.add(self.root_costs[index], charge)?;
                }
            }
        }
        Ok(charge)
    }
    fn multiplier(&mut self, value: &cst::Value) -> Result<Option<u32>, GraphqlError> {
        match value {
            cst::Value::IntValue(i) => Ok(Some(positive_int(
                i32::try_from(i).map_err(|_| GraphqlError::Invalid)?,
            )?)),
            cst::Value::Variable(v) => {
                let n = name(v.name())?;
                let definition = self.vars.get(&n).ok_or(GraphqlError::Invalid)?;
                let ty = TypeRef::parse(definition.ty().ok_or(GraphqlError::Invalid)?)?;
                if !ty.scalar_int() {
                    return Err(GraphqlError::Invalid);
                }
                let default = definition.default_value().and_then(|v| v.value());
                let default = match default {
                    None => None,
                    Some(cst::Value::IntValue(i)) => {
                        Some(positive_int(i32::try_from(i).map_err(|_| GraphqlError::Invalid)?)?)
                    }
                    _ => return Err(GraphqlError::Invalid),
                };
                if let Some(value) = self.values.and_then(|values| values.get(&n)) {
                    return json_int(value).map(Some);
                }
                if let Some(default) = default {
                    return Ok(Some(default));
                }
                if ty.required() {
                    return Err(GraphqlError::Invalid);
                }
                Ok(None)
            }
            _ => Err(GraphqlError::Invalid),
        }
    }
}
fn positive_int(value: i32) -> Result<u32, GraphqlError> {
    if (0..=1_000_000).contains(&value) {
        Ok((value as u32).max(1))
    } else {
        Err(GraphqlError::Invalid)
    }
}
fn json_int(value: &Value) -> Result<u32, GraphqlError> {
    let number = value.as_number().ok_or(GraphqlError::Invalid)?;
    if let Some(value) = number.as_i64() {
        return i32::try_from(value)
            .map_err(|_| GraphqlError::Invalid)
            .and_then(positive_int);
    }
    let value = number.as_f64().ok_or(GraphqlError::Invalid)?;
    if !value.is_finite() || value.fract() != 0.0 || value < i32::MIN as f64 || value > i32::MAX as f64 {
        return Err(GraphqlError::Invalid);
    }
    positive_int(value as i32)
}
