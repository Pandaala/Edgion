use super::{name, parse_document, CompiledGraphqlCost, GraphqlError, Work};
use crate::resources::edgion_plugins::plugin_configs::GraphqlCostRule;
use apollo_parser::cst;
use std::collections::{HashMap, HashSet};

pub(super) use crate::graphql_request::TypeRef;

#[derive(Debug, Clone)]
pub(super) enum DefaultValue {
    Missing,
    Int(i32),
    Null,
    Other,
}
impl DefaultValue {
    fn parse(value: Option<cst::DefaultValue>) -> Result<Self, GraphqlError> {
        match value.and_then(|v| v.value()) {
            None => Ok(Self::Missing),
            Some(cst::Value::IntValue(i)) => i32::try_from(i).map(Self::Int).map_err(|_| GraphqlError::Invalid),
            Some(cst::Value::NullValue(_)) => Ok(Self::Null),
            Some(_) => Ok(Self::Other),
        }
    }
    pub fn absent(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Debug, Clone)]
pub(super) struct Argument {
    pub ty: TypeRef,
    pub default: DefaultValue,
}
#[derive(Debug)]
pub(super) struct Field {
    pub ty: TypeRef,
    pub args: HashMap<String, Argument>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Object,
    Interface,
    Union,
    Scalar,
    Enum,
    Input,
}
#[derive(Debug)]
pub(super) struct TypeDef {
    pub kind: Kind,
    pub fields: HashMap<String, Field>,
    pub inputs: HashMap<String, Argument>,
    pub interfaces: Vec<String>,
    pub possible: Vec<String>,
}
impl TypeDef {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            fields: HashMap::new(),
            inputs: HashMap::new(),
            interfaces: Vec::new(),
            possible: Vec::new(),
        }
    }
    pub fn composite(&self) -> bool {
        matches!(self.kind, Kind::Object | Kind::Interface | Kind::Union)
    }
}
#[derive(Debug)]
pub(super) struct Schema {
    pub types: HashMap<String, TypeDef>,
    pub query: String,
    pub mutation: Option<String>,
}

fn no_directives(d: Option<cst::Directives>) -> Result<(), GraphqlError> {
    if d.is_some() {
        Err(GraphqlError::Invalid)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Metadata {
    Deprecated { required: bool },
    SpecifiedBy,
}

// Validate cold SDL metadata, then discard it. It never affects runtime cost.
fn metadata(d: Option<cst::Directives>, location: Metadata) -> Result<(), GraphqlError> {
    let Some(d) = d else { return Ok(()) };
    let mut directives = d.directives();
    let directive = directives.next().ok_or(GraphqlError::Invalid)?;
    if directives.next().is_some() {
        return Err(GraphqlError::Invalid);
    }
    let (expected, argument, mandatory) = match location {
        Metadata::Deprecated { required: false } => ("deprecated", "reason", false),
        Metadata::Deprecated { required: true } => return Err(GraphqlError::Invalid),
        Metadata::SpecifiedBy => ("specifiedBy", "url", true),
    };
    if name(directive.name())? != expected {
        return Err(GraphqlError::Invalid);
    }
    let mut arguments = directive.arguments().into_iter().flat_map(|a| a.arguments());
    match arguments.next() {
        Some(value) => {
            if name(value.name())? != argument
                || !matches!(value.value(), Some(cst::Value::StringValue(_)))
                || arguments.next().is_some()
            {
                return Err(GraphqlError::Invalid);
            }
        }
        None if mandatory => return Err(GraphqlError::Invalid),
        None => {}
    }
    Ok(())
}
fn argument(input: cst::InputValueDefinition) -> Result<(String, Argument), GraphqlError> {
    let ty = TypeRef::parse(input.ty().ok_or(GraphqlError::Invalid)?)?;
    let default = DefaultValue::parse(input.default_value())?;
    metadata(
        input.directives(),
        Metadata::Deprecated {
            required: ty.required() && default.absent(),
        },
    )?;
    if ty.required() && matches!(default, DefaultValue::Null) {
        return Err(GraphqlError::Invalid);
    }
    if ty.scalar_int() && matches!(default, DefaultValue::Other) {
        return Err(GraphqlError::Invalid);
    }
    Ok((name(input.name())?, Argument { ty, default }))
}
fn fields(
    source: Option<cst::FieldsDefinition>,
    total: &mut usize,
    args_total: &mut usize,
) -> Result<HashMap<String, Field>, GraphqlError> {
    let mut fields = HashMap::new();
    if let Some(source) = source {
        for field in source.field_definitions() {
            metadata(field.directives(), Metadata::Deprecated { required: false })?;
            if *total >= 2048 {
                return Err(GraphqlError::Invalid);
            }
            *total += 1;
            let mut args = HashMap::new();
            if let Some(defs) = field.arguments_definition() {
                for input in defs.input_value_definitions() {
                    if *args_total >= 4096 {
                        return Err(GraphqlError::Invalid);
                    }
                    *args_total += 1;
                    let (n, a) = argument(input)?;
                    if args.insert(n, a).is_some() {
                        return Err(GraphqlError::Invalid);
                    }
                }
            }
            let n = name(field.name())?;
            let ty = TypeRef::parse(field.ty().ok_or(GraphqlError::Invalid)?)?;
            if fields.insert(n, Field { ty, args }).is_some() {
                return Err(GraphqlError::Invalid);
            }
        }
    }
    Ok(fields)
}
fn interfaces(source: Option<cst::ImplementsInterfaces>, membership: &mut usize) -> Result<Vec<String>, GraphqlError> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    if let Some(source) = source {
        for ty in source.named_types() {
            if *membership >= 4096 {
                return Err(GraphqlError::Invalid);
            }
            *membership += 1;
            let n = name(ty.name())?;
            if !seen.insert(n.clone()) {
                return Err(GraphqlError::Invalid);
            }
            result.push(n);
        }
    }
    Ok(result)
}

impl Schema {
    fn compile(source: &str) -> Result<(Self, Work), GraphqlError> {
        let doc = parse_document(source)?;
        let mut types = HashMap::new();
        for n in ["Int", "Float", "String", "Boolean", "ID"] {
            types.insert(n.to_string(), TypeDef::new(Kind::Scalar));
        }
        let (mut fields_total, mut args_total, mut membership) = (0, 0, 0);
        let mut roots = HashMap::new();
        let mut explicit = false;
        for definition in doc.definitions() {
            let (n, t) = match definition {
                cst::Definition::SchemaDefinition(d) => {
                    if explicit {
                        return Err(GraphqlError::Invalid);
                    }
                    explicit = true;
                    no_directives(d.directives())?;
                    for root in d.root_operation_type_definitions() {
                        let op = root.operation_type().ok_or(GraphqlError::Invalid)?;
                        let key = if op.query_token().is_some() {
                            "query"
                        } else if op.mutation_token().is_some() {
                            "mutation"
                        } else {
                            "subscription"
                        };
                        let n = name(root.named_type().and_then(|t| t.name()))?;
                        if roots.insert(key, n).is_some() {
                            return Err(GraphqlError::Invalid);
                        }
                    }
                    continue;
                }
                cst::Definition::ObjectTypeDefinition(d) => {
                    no_directives(d.directives())?;
                    let mut t = TypeDef::new(Kind::Object);
                    t.fields = fields(d.fields_definition(), &mut fields_total, &mut args_total)?;
                    t.interfaces = interfaces(d.implements_interfaces(), &mut membership)?;
                    (name(d.name())?, t)
                }
                cst::Definition::InterfaceTypeDefinition(d) => {
                    no_directives(d.directives())?;
                    if d.implements_interfaces().is_some() {
                        return Err(GraphqlError::Invalid);
                    }
                    let mut t = TypeDef::new(Kind::Interface);
                    t.fields = fields(d.fields_definition(), &mut fields_total, &mut args_total)?;
                    (name(d.name())?, t)
                }
                cst::Definition::UnionTypeDefinition(d) => {
                    no_directives(d.directives())?;
                    let mut t = TypeDef::new(Kind::Union);
                    let mut seen = HashSet::new();
                    if let Some(members) = d.union_member_types() {
                        for member in members.named_types() {
                            if membership >= 4096 {
                                return Err(GraphqlError::Invalid);
                            }
                            membership += 1;
                            let n = name(member.name())?;
                            if !seen.insert(n.clone()) {
                                return Err(GraphqlError::Invalid);
                            }
                            t.possible.push(n);
                        }
                    }
                    (name(d.name())?, t)
                }
                cst::Definition::ScalarTypeDefinition(d) => {
                    metadata(d.directives(), Metadata::SpecifiedBy)?;
                    (name(d.name())?, TypeDef::new(Kind::Scalar))
                }
                cst::Definition::EnumTypeDefinition(d) => {
                    no_directives(d.directives())?;
                    let mut values = HashSet::new();
                    if let Some(defs) = d.enum_values_definition() {
                        for value in defs.enum_value_definitions() {
                            metadata(value.directives(), Metadata::Deprecated { required: false })?;
                            if values.len() >= 256 || !values.insert(name(value.enum_value().and_then(|e| e.name()))?) {
                                return Err(GraphqlError::Invalid);
                            }
                        }
                    }
                    (name(d.name())?, TypeDef::new(Kind::Enum))
                }
                cst::Definition::InputObjectTypeDefinition(d) => {
                    no_directives(d.directives())?;
                    let mut t = TypeDef::new(Kind::Input);
                    if let Some(defs) = d.input_fields_definition() {
                        for input in defs.input_value_definitions() {
                            if fields_total >= 2048 {
                                return Err(GraphqlError::Invalid);
                            }
                            fields_total += 1;
                            let (n, a) = argument(input)?;
                            if t.inputs.insert(n, a).is_some() {
                                return Err(GraphqlError::Invalid);
                            }
                        }
                    }
                    (name(d.name())?, t)
                }
                _ => return Err(GraphqlError::Invalid),
            };
            if types.len() >= 256 || n.starts_with("__") || types.insert(n, t).is_some() {
                return Err(GraphqlError::Invalid);
            }
        }
        let query = if explicit {
            roots.get("query").cloned().ok_or(GraphqlError::Invalid)?
        } else {
            "Query".to_string()
        };
        let mutation = if explicit {
            roots.get("mutation").cloned()
        } else {
            types.contains_key("Mutation").then(|| "Mutation".to_string())
        };
        for n in std::iter::once(&query)
            .chain(mutation.iter())
            .chain(roots.get("subscription"))
        {
            if types.get(n).is_none_or(|t| t.kind != Kind::Object) {
                return Err(GraphqlError::Invalid);
            }
        }
        let mut work = Work::default();
        let mut memberships: HashMap<String, Vec<String>> = HashMap::new();
        for (n, t) in &types {
            work.step()?;
            if t.kind == Kind::Object {
                for interface in &t.interfaces {
                    work.step()?;
                    if types.get(interface).is_none_or(|i| i.kind != Kind::Interface) {
                        return Err(GraphqlError::Invalid);
                    }
                    memberships.entry(interface.clone()).or_default().push(n.clone());
                }
            }
            if t.kind == Kind::Union {
                for n in &t.possible {
                    work.step()?;
                    if types.get(n).is_none_or(|t| t.kind != Kind::Object) {
                        return Err(GraphqlError::Invalid);
                    }
                }
            }
            for field in t.fields.values() {
                work.step()?;
                if types.get(&field.ty.name).is_none_or(|t| t.kind == Kind::Input) {
                    return Err(GraphqlError::Invalid);
                }
                for arg in field.args.values() {
                    work.step()?;
                    check_input(&types, &arg.ty)?;
                }
            }
            for input in t.inputs.values() {
                work.step()?;
                check_input(&types, &input.ty)?;
            }
        }
        for (n, t) in &mut types {
            if t.kind == Kind::Object {
                t.possible.push(n.clone());
            }
            if t.kind == Kind::Interface {
                t.possible = memberships.remove(n).unwrap_or_default();
            }
        }
        for t in types.values().filter(|t| t.kind == Kind::Object) {
            for iface in &t.interfaces {
                let contract = &types[iface];
                for (n, expected) in &contract.fields {
                    work.step()?;
                    let actual = t.fields.get(n).ok_or(GraphqlError::Invalid)?;
                    if !covariant(&types, &actual.ty, &expected.ty, &mut work)? {
                        return Err(GraphqlError::Invalid);
                    }
                    for (n, arg) in &expected.args {
                        work.step()?;
                        if actual.args.get(n).is_none_or(|a| a.ty != arg.ty) {
                            return Err(GraphqlError::Invalid);
                        }
                    }
                    for (n, arg) in &actual.args {
                        work.step()?;
                        if !expected.args.contains_key(n) && arg.ty.required() && arg.default.absent() {
                            return Err(GraphqlError::Invalid);
                        }
                    }
                }
            }
        }
        Ok((Self { types, query, mutation }, work))
    }
}
fn check_input(types: &HashMap<String, TypeDef>, ty: &TypeRef) -> Result<(), GraphqlError> {
    if types
        .get(&ty.name)
        .is_some_and(|t| matches!(t.kind, Kind::Scalar | Kind::Enum | Kind::Input))
    {
        Ok(())
    } else {
        Err(GraphqlError::Invalid)
    }
}
fn covariant(
    types: &HashMap<String, TypeDef>,
    actual: &TypeRef,
    expected: &TypeRef,
    work: &mut Work,
) -> Result<bool, GraphqlError> {
    if actual.nulls.len() != expected.nulls.len() {
        return Ok(false);
    }
    for (a, e) in actual.nulls.iter().zip(&expected.nulls) {
        work.step()?;
        if *e && !*a {
            return Ok(false);
        }
    }
    if actual.name == expected.name {
        return Ok(true);
    }
    let a = &types[&actual.name];
    let e = &types[&expected.name];
    if !matches!(e.kind, Kind::Interface | Kind::Union) || a.kind != Kind::Object {
        return Ok(false);
    }
    for possible in &a.possible {
        work.step()?;
        let mut found = false;
        for allowed in &e.possible {
            work.step()?;
            if allowed == possible {
                found = true;
                break;
            }
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(!a.possible.is_empty())
}

impl CompiledGraphqlCost {
    pub fn compile(source: &str, rules: &[GraphqlCostRule], max_cost: u32, max_depth: usize) -> Result<Self, String> {
        Self::compile_inner(source, rules, max_cost, max_depth)
            .map_err(|_| "invalid or unsupported GraphQL cost configuration".to_string())
    }
    fn compile_inner(
        source: &str,
        rules: &[GraphqlCostRule],
        max_cost: u32,
        max_depth: usize,
    ) -> Result<Self, GraphqlError> {
        if !(1..=1_000_000).contains(&max_cost) || !(1..=32).contains(&max_depth) || rules.len() > 256 {
            return Err(GraphqlError::Invalid);
        }
        let (schema, mut work) = Schema::compile(source)?;
        let mut compiled = HashMap::new();
        for rule in rules {
            work.step()?;
            if !(1..=1_000_000).contains(&rule.weight)
                || rule.default_multiplier > 1_000_000
                || rule.multipliers.len() > 8
            {
                return Err(GraphqlError::Invalid);
            }
            let (ty, field) = rule.field.split_once('.').ok_or(GraphqlError::Invalid)?;
            let ty = schema
                .types
                .get(ty)
                .filter(|t| matches!(t.kind, Kind::Object | Kind::Interface))
                .ok_or(GraphqlError::Invalid)?;
            let field = ty.fields.get(field).ok_or(GraphqlError::Invalid)?;
            if !rule.multipliers.is_empty() && !schema.types[&field.ty.name].composite() {
                return Err(GraphqlError::Invalid);
            }
            let mut names = HashSet::new();
            for n in &rule.multipliers {
                work.step()?;
                let arg = field.args.get(n).ok_or(GraphqlError::Invalid)?;
                if !names.insert(n) || !arg.ty.scalar_int() {
                    return Err(GraphqlError::Invalid);
                }
                match arg.default {
                    DefaultValue::Missing => {}
                    DefaultValue::Int(i) if (0..=1_000_000).contains(&i) => {}
                    _ => return Err(GraphqlError::Invalid),
                }
                // The concrete field's default wins even for an interface rule.
                for concrete in &ty.possible {
                    work.step()?;
                    let a = &schema.types[concrete].fields[rule.field.split_once('.').ok_or(GraphqlError::Invalid)?.1]
                        .args[n];
                    match a.default {
                        DefaultValue::Missing => {}
                        DefaultValue::Int(i) if (0..=1_000_000).contains(&i) => {}
                        _ => return Err(GraphqlError::Invalid),
                    }
                }
            }
            if compiled.insert(rule.field.clone(), rule.clone()).is_some() {
                return Err(GraphqlError::Invalid);
            }
        }
        Ok(Self {
            schema,
            rules: compiled,
            max_cost,
            max_depth,
            root_fields: Vec::new(),
        })
    }
}
