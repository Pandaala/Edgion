use super::schema::{DefaultValue, TypeRef};
use super::{name, parse_document, CompiledGraphqlCost, GraphqlCostError, Work, MAX_ENVELOPE_BYTES, MAX_RECURSION};
use crate::resources::edgion_plugins::plugin_configs::parse_json_instance_with_limits;
use apollo_parser::cst;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

struct Envelope<'a> {
    query: &'a str,
    operation: Option<&'a str>,
    variables: Option<&'a Map<String, Value>>,
}
fn envelope(value: &Value) -> Result<Envelope<'_>, GraphqlCostError> {
    let object = value.as_object().ok_or(GraphqlCostError::Invalid)?;
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "query" | "operationName" | "variables" | "extensions"))
    {
        return Err(GraphqlCostError::Invalid);
    }
    let query = object
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
        .ok_or(GraphqlCostError::Invalid)?;
    let operation = match object.get("operationName") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        _ => return Err(GraphqlCostError::Invalid),
    };
    let variables = match object.get("variables") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) => Some(m),
        _ => return Err(GraphqlCostError::Invalid),
    };
    match object.get("extensions") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) if m.is_empty() => {}
        _ => return Err(GraphqlCostError::Invalid),
    }
    Ok(Envelope {
        query,
        operation,
        variables,
    })
}
fn strict_json(bytes: &[u8]) -> Result<Value, GraphqlCostError> {
    parse_json_instance_with_limits(bytes, 32, 4096)
        .map(|p| p.into_value())
        .map_err(|_| GraphqlCostError::Invalid)
}
fn decode(source: &str, remaining: &mut usize) -> Result<String, GraphqlCostError> {
    let source = source.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if *remaining == 0 {
            return Err(GraphqlCostError::TooLarge);
        }
        let byte = match source[i] {
            b'+' => b' ',
            b'%' => {
                fn hex(b: u8) -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                }
                let a = source
                    .get(i + 1)
                    .and_then(|b| hex(*b))
                    .ok_or(GraphqlCostError::Invalid)?;
                let b = source
                    .get(i + 2)
                    .and_then(|b| hex(*b))
                    .ok_or(GraphqlCostError::Invalid)?;
                i += 2;
                a * 16 + b
            }
            b => b,
        };
        result.push(byte);
        *remaining -= 1;
        i += 1;
    }
    String::from_utf8(result).map_err(|_| GraphqlCostError::Invalid)
}

impl CompiledGraphqlCost {
    pub fn estimate_post(&self, bytes: &[u8], limit: usize) -> Result<u32, GraphqlCostError> {
        if bytes.len() > limit.min(MAX_ENVELOPE_BYTES) {
            return Err(GraphqlCostError::TooLarge);
        }
        let value = strict_json(bytes)?;
        self.estimate(envelope(&value)?, false)
    }
    pub fn estimate_get(&self, query: &str, limit: usize) -> Result<u32, GraphqlCostError> {
        let limit = limit.min(MAX_ENVELOPE_BYTES);
        if query.len() > limit {
            return Err(GraphqlCostError::TooLarge);
        }
        let mut remaining = limit;
        let mut params = Map::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(GraphqlCostError::Invalid)?;
            let key = decode(key, &mut remaining)?;
            if !matches!(key.as_str(), "query" | "operationName" | "variables" | "extensions")
                || params.contains_key(&key)
            {
                return Err(GraphqlCostError::Invalid);
            }
            let value = decode(value, &mut remaining)?;
            let value = if matches!(key.as_str(), "variables" | "extensions") {
                strict_json(value.as_bytes())?
            } else {
                Value::String(value)
            };
            params.insert(key, value);
        }
        let value = Value::Object(params);
        self.estimate(envelope(&value)?, true)
    }
    fn estimate(&self, envelope: Envelope<'_>, get: bool) -> Result<u32, GraphqlCostError> {
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
                    if operations.len() >= super::MAX_TOKENS {
                        return Err(GraphqlCostError::Invalid);
                    }
                    directives(op.directives(), &mut work)?;
                    if let Some(n) = op.name() {
                        if !operation_names.insert(n.text().to_string()) {
                            return Err(GraphqlCostError::Invalid);
                        }
                    } else {
                        anonymous = true;
                    }
                    operations.push(op);
                }
                cst::Definition::FragmentDefinition(f) => {
                    if fragments.len() >= super::MAX_TOKENS {
                        return Err(GraphqlCostError::Invalid);
                    }
                    directives(f.directives(), &mut work)?;
                    let n = name(f.fragment_name().and_then(|n| n.name()))?;
                    if fragments.insert(n, f).is_some() {
                        return Err(GraphqlCostError::Invalid);
                    }
                }
                _ => return Err(GraphqlCostError::Invalid),
            }
        }
        if operations.is_empty() || (anonymous && operations.len() != 1) {
            return Err(GraphqlCostError::Invalid);
        }
        let operation = if let Some(n) = envelope.operation {
            operations
                .iter()
                .find(|o| o.name().is_some_and(|name| name.text() == n))
                .ok_or(GraphqlCostError::Invalid)?
        } else {
            if operations.len() != 1 {
                return Err(GraphqlCostError::Invalid);
            }
            &operations[0]
        };
        directives(operation.directives(), &mut work)?;
        let ty = operation.operation_type();
        if ty.as_ref().is_some_and(|o| o.subscription_token().is_some()) {
            return Err(GraphqlCostError::Invalid);
        }
        let root = if ty.is_some_and(|o| o.mutation_token().is_some()) {
            if get {
                return Err(GraphqlCostError::MethodNotAllowed);
            }
            self.schema.mutation.as_ref().ok_or(GraphqlCostError::Invalid)?
        } else {
            &self.schema.query
        };
        let mut vars = HashMap::new();
        if let Some(defs) = operation.variable_definitions() {
            for def in defs.variable_definitions() {
                work.step()?;
                if vars.len() >= super::MAX_TOKENS {
                    return Err(GraphqlCostError::Invalid);
                }
                directives(def.directives(), &mut work)?;
                let n = name(def.variable().and_then(|v| v.name()))?;
                if vars.insert(n, def).is_some() {
                    return Err(GraphqlCostError::Invalid);
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
                &op.selection_set().ok_or(GraphqlCostError::Invalid)?,
                &fragments,
                &mut states,
                &mut work,
                0,
            )?;
        }
        let mut evaluation = Evaluation {
            model: self,
            fragments: &fragments,
            vars: &vars,
            values: envelope.variables,
            work,
        };
        evaluation.validate_set(&operation.selection_set().ok_or(GraphqlCostError::Invalid)?, root, 0)?;
        for f in fragments.values() {
            let ty = name(f.type_condition().and_then(|t| t.named_type()).and_then(|t| t.name()))?;
            evaluation.validate_set(&f.selection_set().ok_or(GraphqlCostError::Invalid)?, &ty, 0)?;
        }
        let cost = evaluation.set(
            &operation.selection_set().ok_or(GraphqlCostError::Invalid)?,
            root,
            root,
            1,
            0,
        )?;
        evaluation.add(1, cost)
    }
}

fn directives(source: Option<cst::Directives>, work: &mut Work) -> Result<(), GraphqlCostError> {
    if let Some(source) = source {
        for directive in source.directives() {
            work.step()?;
            let n = name(directive.name())?;
            if !matches!(n.as_str(), "skip" | "include") {
                return Err(GraphqlCostError::Invalid);
            }
            let args = directive.arguments().ok_or(GraphqlCostError::Invalid)?;
            let mut count = 0;
            for arg in args.arguments() {
                work.step()?;
                count += 1;
                if name(arg.name())? != "if"
                    || !matches!(arg.value(), Some(cst::Value::BooleanValue(_) | cst::Value::Variable(_)))
                {
                    return Err(GraphqlCostError::Invalid);
                }
            }
            if count != 1 {
                return Err(GraphqlCostError::Invalid);
            }
        }
    }
    Ok(())
}
fn graph_fragment(
    n: &str,
    fragments: &HashMap<String, cst::FragmentDefinition>,
    states: &mut HashMap<String, u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), GraphqlCostError> {
    work.step()?;
    if depth >= MAX_RECURSION {
        return Err(GraphqlCostError::Invalid);
    }
    match states.get(n) {
        Some(1) => return Err(GraphqlCostError::Invalid),
        Some(2) => return Ok(()),
        _ => {}
    }
    let fragment = fragments.get(n).ok_or(GraphqlCostError::Invalid)?;
    states.insert(n.to_string(), 1);
    graph_set(
        &fragment.selection_set().ok_or(GraphqlCostError::Invalid)?,
        fragments,
        states,
        work,
        depth + 1,
    )?;
    states.insert(n.to_string(), 2);
    Ok(())
}
fn graph_set(
    set: &cst::SelectionSet,
    fragments: &HashMap<String, cst::FragmentDefinition>,
    states: &mut HashMap<String, u8>,
    work: &mut Work,
    depth: usize,
) -> Result<(), GraphqlCostError> {
    if depth >= MAX_RECURSION {
        return Err(GraphqlCostError::Invalid);
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
                    &f.selection_set().ok_or(GraphqlCostError::Invalid)?,
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
                        if names.len() >= super::MAX_TOKENS || !names.insert(name(arg.name())?) {
                            return Err(GraphqlCostError::Invalid);
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

struct Evaluation<'a> {
    model: &'a CompiledGraphqlCost,
    fragments: &'a HashMap<String, cst::FragmentDefinition>,
    vars: &'a HashMap<String, cst::VariableDefinition>,
    values: Option<&'a Map<String, Value>>,
    work: Work,
}
impl Evaluation<'_> {
    // Validate static selection shapes without following or expanding spreads.
    // Fragment bodies are checked once each in their declared context.
    fn validate_set(&mut self, set: &cst::SelectionSet, ty: &str, recursion: usize) -> Result<(), GraphqlCostError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlCostError::Invalid);
        }
        let source = self
            .model
            .schema
            .types
            .get(ty)
            .filter(|t| t.composite())
            .ok_or(GraphqlCostError::Invalid)?;
        for selection in set.selections() {
            self.work.step()?;
            match selection {
                cst::Selection::Field(f) => {
                    let n = name(f.name())?;
                    if n == "__typename" {
                        if f.arguments().is_some() || f.selection_set().is_some() {
                            return Err(GraphqlCostError::Invalid);
                        }
                        continue;
                    }
                    if n.starts_with("__") {
                        return Err(GraphqlCostError::Invalid);
                    }
                    let field = source.fields.get(&n).ok_or(GraphqlCostError::Invalid)?;
                    if let Some(args) = f.arguments() {
                        for arg in args.arguments() {
                            self.work.step()?;
                            if !field.args.contains_key(&name(arg.name())?) {
                                return Err(GraphqlCostError::Invalid);
                            }
                        }
                    }
                    let returned = &self.model.schema.types[&field.ty.name];
                    match (returned.composite(), f.selection_set()) {
                        (true, Some(set)) => self.validate_set(&set, &field.ty.name, recursion + 1)?,
                        (false, None) => {}
                        _ => return Err(GraphqlCostError::Invalid),
                    }
                }
                cst::Selection::InlineFragment(f) => {
                    let target = match f.type_condition() {
                        Some(t) => name(t.named_type().and_then(|t| t.name()))?,
                        None => ty.to_string(),
                    };
                    self.condition(ty, "", &target)?;
                    self.validate_set(
                        &f.selection_set().ok_or(GraphqlCostError::Invalid)?,
                        &target,
                        recursion + 1,
                    )?;
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
    fn add(&self, a: u32, b: u32) -> Result<u32, GraphqlCostError> {
        a.checked_add(b)
            .filter(|n| *n <= self.model.max_cost)
            .ok_or(GraphqlCostError::Invalid)
    }
    fn set(
        &mut self,
        set: &cst::SelectionSet,
        static_ty: &str,
        runtime_ty: &str,
        depth: usize,
        recursion: usize,
    ) -> Result<u32, GraphqlCostError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlCostError::Invalid);
        }
        let t = self
            .model
            .schema
            .types
            .get(runtime_ty)
            .filter(|t| t.composite())
            .ok_or(GraphqlCostError::Invalid)?;
        if t.possible.is_empty() {
            return Err(GraphqlCostError::Invalid);
        }
        let mut maximum = 0;
        for object in &t.possible {
            self.work.step()?;
            maximum = maximum.max(self.object_set(set, static_ty, object, depth, recursion + 1)?);
        }
        Ok(maximum)
    }
    fn condition(&mut self, static_ty: &str, object: &str, condition: &str) -> Result<bool, GraphqlCostError> {
        let source = &self.model.schema.types[static_ty];
        let target = self
            .model
            .schema
            .types
            .get(condition)
            .filter(|t| t.composite())
            .ok_or(GraphqlCostError::Invalid)?;
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
            return Err(GraphqlCostError::Invalid);
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
    ) -> Result<u32, GraphqlCostError> {
        if recursion >= MAX_RECURSION {
            return Err(GraphqlCostError::Invalid);
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
                            &f.selection_set().ok_or(GraphqlCostError::Invalid)?,
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
                    let f = self.fragments.get(&n).ok_or(GraphqlCostError::Invalid)?;
                    let ty = name(f.type_condition().and_then(|t| t.named_type()).and_then(|t| t.name()))?;
                    if self.condition(static_ty, object, &ty)? {
                        self.object_set(
                            &f.selection_set().ok_or(GraphqlCostError::Invalid)?,
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
    ) -> Result<u32, GraphqlCostError> {
        if depth > self.model.max_depth {
            return Err(GraphqlCostError::Invalid);
        }
        directives(field.directives(), &mut self.work)?;
        let n = name(field.name())?;
        if n == "__typename" {
            if field.selection_set().is_some() || field.arguments().is_some() {
                return Err(GraphqlCostError::Invalid);
            }
            return Ok(1);
        }
        if n.starts_with("__") || !self.model.schema.types[static_ty].fields.contains_key(&n) {
            return Err(GraphqlCostError::Invalid);
        }
        let declared = &self.model.schema.types[static_ty].fields[&n];
        let actual = self.model.schema.types[object]
            .fields
            .get(&n)
            .ok_or(GraphqlCostError::Invalid)?;
        let mut args = HashMap::new();
        if let Some(arguments) = field.arguments() {
            for arg in arguments.arguments() {
                self.work.step()?;
                if args.len() >= super::MAX_TOKENS {
                    return Err(GraphqlCostError::Invalid);
                }
                let key = name(arg.name())?;
                if !actual.args.contains_key(&key)
                    || args
                        .insert(key, arg.value().ok_or(GraphqlCostError::Invalid)?)
                        .is_some()
                {
                    return Err(GraphqlCostError::Invalid);
                }
            }
        }
        let return_type = &self.model.schema.types[&actual.ty.name];
        let children = match (return_type.composite(), field.selection_set()) {
            (true, Some(set)) => self.set(&set, &declared.ty.name, &actual.ty.name, depth + 1, recursion)?,
            (false, None) => 0,
            _ => return Err(GraphqlCostError::Invalid),
        };
        let concrete = &self.model.schema.types[object];
        let mut cost = None;
        for coordinate in std::iter::once(object).chain(concrete.interfaces.iter().map(String::as_str)) {
            self.work.step()?;
            let coordinate = format!("{coordinate}.{n}");
            if let Some(rule) = self.model.rules.get(&coordinate) {
                let mut factor = 1u32;
                for argument in &rule.multipliers {
                    self.work.step()?;
                    let supplied = args.get(argument).map(|v| self.multiplier(v)).transpose()?.flatten();
                    let value = match supplied {
                        Some(value) => value,
                        None => match actual.args[argument].default {
                            DefaultValue::Int(i) => positive_int(i)?,
                            DefaultValue::Missing => rule.default_multiplier.max(1),
                            _ => return Err(GraphqlCostError::Invalid),
                        },
                    };
                    factor = factor.checked_mul(value).ok_or(GraphqlCostError::Invalid)?;
                }
                let candidate = self.add(
                    rule.weight,
                    factor.checked_mul(children).ok_or(GraphqlCostError::Invalid)?,
                )?;
                cost = Some(cost.unwrap_or(0u32).max(candidate));
            }
        }
        cost.map(Ok).unwrap_or_else(|| self.add(1, children))
    }
    fn multiplier(&mut self, value: &cst::Value) -> Result<Option<u32>, GraphqlCostError> {
        match value {
            cst::Value::IntValue(i) => Ok(Some(positive_int(
                i32::try_from(i).map_err(|_| GraphqlCostError::Invalid)?,
            )?)),
            cst::Value::Variable(v) => {
                let n = name(v.name())?;
                let definition = self.vars.get(&n).ok_or(GraphqlCostError::Invalid)?;
                let ty = TypeRef::parse(definition.ty().ok_or(GraphqlCostError::Invalid)?)?;
                if !ty.scalar_int() {
                    return Err(GraphqlCostError::Invalid);
                }
                let default = definition.default_value().and_then(|v| v.value());
                let default = match default {
                    None => None,
                    Some(cst::Value::IntValue(i)) => {
                        Some(positive_int(i32::try_from(i).map_err(|_| GraphqlCostError::Invalid)?)?)
                    }
                    _ => return Err(GraphqlCostError::Invalid),
                };
                if let Some(value) = self.values.and_then(|values| values.get(&n)) {
                    return json_int(value).map(Some);
                }
                if let Some(default) = default {
                    return Ok(Some(default));
                }
                if ty.required() {
                    return Err(GraphqlCostError::Invalid);
                }
                Ok(None)
            }
            _ => Err(GraphqlCostError::Invalid),
        }
    }
}
fn positive_int(value: i32) -> Result<u32, GraphqlCostError> {
    if (0..=1_000_000).contains(&value) {
        Ok((value as u32).max(1))
    } else {
        Err(GraphqlCostError::Invalid)
    }
}
fn json_int(value: &Value) -> Result<u32, GraphqlCostError> {
    let number = value.as_number().ok_or(GraphqlCostError::Invalid)?;
    if let Some(value) = number.as_i64() {
        return i32::try_from(value)
            .map_err(|_| GraphqlCostError::Invalid)
            .and_then(positive_int);
    }
    let value = number.as_f64().ok_or(GraphqlCostError::Invalid)?;
    if !value.is_finite() || value.fract() != 0.0 || value < i32::MIN as f64 || value > i32::MAX as f64 {
        return Err(GraphqlCostError::Invalid);
    }
    positive_int(value as i32)
}
