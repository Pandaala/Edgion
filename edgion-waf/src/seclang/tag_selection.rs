//! Bounded configuration-only selection of complete semantic rule units.

use std::borrow::Cow;
use std::fmt;
use std::mem::size_of;

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::compiler::CanonicalWork;
use super::{
    ActionEnvelopeKind, ActionKind, ActionOperandView, CompilerError, CompilerLimits, ValidatedRuleUnitView,
    ValidatedSemanticDocument,
};

pub const MAX_SELECTION_NODES: usize = 256;
pub const MAX_SELECTION_LEAVES: usize = 64;
pub const MAX_SELECTION_DEPTH: usize = 16;
pub const MAX_SELECTION_TAG_BYTES: usize = 256;
pub const MAX_SELECTION_BYTES: usize = 16 * 1024;
pub const MAX_POLICY_SELECTION_BYTES: usize = 64 * 1024;

/// Inclusive interval of canonical root rule IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WafRuleIdRange {
    pub start: u64,
    pub end: u64,
}

/// One Boolean literal or one-key operator map. Nodes cannot be constructed unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WafRuleSelectionExpr {
    node: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Constant(bool),
    Tag(String),
    RuleId(u64),
    RuleIdRange(WafRuleIdRange),
    Phase(u8),
    AllOf(Vec<WafRuleSelectionExpr>),
    AnyOf(Vec<WafRuleSelectionExpr>),
    // A bounded single-element arena permits fallible allocation without Box::new.
    Not(Vec<WafRuleSelectionExpr>),
}

/// Static membership filters. Missing include/exclude mean true/false respectively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafRuleSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "WafRuleSelectionExpr")]
    include: Option<WafRuleSelectionExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "WafRuleSelectionExpr")]
    exclude: Option<WafRuleSelectionExpr>,
    #[serde(skip)]
    #[schemars(skip)]
    stats: WafSelectionStats,
}

impl Default for WafRuleSelection {
    fn default() -> Self {
        Self {
            include: None,
            exclude: None,
            stats: WafSelectionStats {
                encoded_bytes: 2,
                owned_bytes: size_of::<Self>(),
                ..Default::default()
            },
        }
    }
}

/// Deterministic shape and allocation accounting, independent of a source corpus.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WafSelectionStats {
    pub nodes: usize,
    pub metadata_leaves: usize,
    pub encoded_bytes: usize,
    pub owned_bytes: usize,
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum WafSelectionError {
    #[error("invalid WAF rule-selection shape or value")]
    Invalid,
    #[error("WAF rule selection exceeds its configured bound")]
    Limit,
    #[error("cannot allocate bounded WAF rule selection")]
    Allocation,
}

impl WafRuleSelection {
    pub const fn is_empty(&self) -> bool {
        self.include.is_none() && self.exclude.is_none()
    }

    /// Validate programmatic inputs and charge the exact compact JSON representation.
    pub fn validate(&self) -> Result<WafSelectionStats, WafSelectionError> {
        Ok(self.stats)
    }

    fn compute_stats(&self) -> Result<WafSelectionStats, WafSelectionError> {
        let mut budget = DecodeBudget::default();
        budget.stats.owned_bytes = size_of::<Self>();
        budget.bytes(2)?;
        for (fields, expression) in [&self.include, &self.exclude].into_iter().flatten().enumerate() {
            budget.bytes(10 + usize::from(fields != 0))?;
            expression.check(1, &mut budget)?;
        }
        Ok(budget.stats)
    }
}

impl WafRuleSelectionExpr {
    fn check(&self, depth: usize, budget: &mut DecodeBudget) -> Result<(), WafSelectionError> {
        budget.node(depth)?;
        match &self.node {
            Node::Constant(value) => budget.bytes(if *value { 4 } else { 5 })?,
            Node::Tag(tag) => {
                budget.leaf()?;
                check_tag(tag)?;
                budget.bytes(10 + tag.len() + tag.bytes().filter(|b| matches!(b, b'"' | b'\\')).count())?;
                budget.stats.owned_bytes = checked_sum(budget.stats.owned_bytes, tag.capacity())?;
            }
            Node::RuleId(id) => {
                budget.leaf()?;
                positive(*id)?;
                budget.bytes(11 + digits(*id))?;
            }
            Node::RuleIdRange(range) => {
                budget.leaf()?;
                valid_range(*range)?;
                budget.bytes(33 + digits(range.start) + digits(range.end))?;
            }
            Node::Phase(phase) => {
                budget.leaf()?;
                valid_phase(*phase)?;
                budget.bytes(11)?;
            }
            Node::AllOf(children) | Node::AnyOf(children) | Node::Not(children) => {
                let key = match self.node {
                    Node::AllOf(_) => "allOf",
                    Node::AnyOf(_) => "anyOf",
                    _ => "not",
                };
                if children.is_empty() || (key == "not" && children.len() != 1) {
                    return Err(WafSelectionError::Invalid);
                }
                budget.bytes(key.len() + 5 + if key == "not" { 0 } else { 1 + children.len() })?;
                budget.stats.owned_bytes = checked_sum(
                    budget.stats.owned_bytes,
                    children
                        .capacity()
                        .checked_mul(size_of::<Self>())
                        .ok_or(WafSelectionError::Limit)?,
                )?;
                for child in children {
                    child.check(depth + 1, budget)?;
                }
            }
        }
        Ok(())
    }
}

fn digits(value: u64) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}
fn checked_sum(a: usize, b: usize) -> Result<usize, WafSelectionError> {
    a.checked_add(b).ok_or(WafSelectionError::Limit)
}
fn positive(value: u64) -> Result<(), WafSelectionError> {
    if value == 0 {
        Err(WafSelectionError::Invalid)
    } else {
        Ok(())
    }
}
fn valid_range(value: WafRuleIdRange) -> Result<(), WafSelectionError> {
    positive(value.start)?;
    if value.start > value.end {
        Err(WafSelectionError::Invalid)
    } else {
        Ok(())
    }
}
fn valid_phase(value: u8) -> Result<(), WafSelectionError> {
    if (1..=5).contains(&value) {
        Ok(())
    } else {
        Err(WafSelectionError::Invalid)
    }
}
fn check_tag(value: &str) -> Result<(), WafSelectionError> {
    if value.len() > MAX_SELECTION_TAG_BYTES {
        Err(WafSelectionError::Limit)
    } else if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(WafSelectionError::Invalid)
    } else {
        Ok(())
    }
}

#[derive(Default)]
struct DecodeBudget {
    stats: WafSelectionStats,
}
impl DecodeBudget {
    fn node(&mut self, depth: usize) -> Result<(), WafSelectionError> {
        if depth > MAX_SELECTION_DEPTH || self.stats.nodes == MAX_SELECTION_NODES {
            return Err(WafSelectionError::Limit);
        }
        self.stats.nodes += 1;
        Ok(())
    }
    fn leaf(&mut self) -> Result<(), WafSelectionError> {
        if self.stats.metadata_leaves == MAX_SELECTION_LEAVES {
            return Err(WafSelectionError::Limit);
        }
        self.stats.metadata_leaves += 1;
        Ok(())
    }
    fn bytes(&mut self, bytes: usize) -> Result<(), WafSelectionError> {
        self.stats.encoded_bytes = checked_sum(self.stats.encoded_bytes, bytes)?;
        if self.stats.encoded_bytes > MAX_SELECTION_BYTES {
            Err(WafSelectionError::Limit)
        } else {
            Ok(())
        }
    }
}

fn invalid<E: de::Error>() -> E {
    E::custom(WafSelectionError::Invalid)
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum Key {
    Include,
    Exclude,
    Tag,
    RuleId,
    RuleIdRange,
    Phase,
    AllOf,
    AnyOf,
    Not,
}

struct ExprSeed<'a> {
    budget: &'a mut DecodeBudget,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for ExprSeed<'_> {
    type Value = WafRuleSelectionExpr;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        self.budget.node(self.depth).map_err(de::Error::custom)?;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for ExprSeed<'_> {
    type Value = WafRuleSelectionExpr;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a Boolean or one-key WAF metadata condition")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        self.budget.bytes(if value { 4 } else { 5 }).map_err(E::custom)?;
        Ok(WafRuleSelectionExpr {
            node: Node::Constant(value),
        })
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let key = map.next_key::<Key>()?.ok_or_else(invalid)?;
        let node = match key {
            Key::Tag => {
                self.budget.leaf().map_err(de::Error::custom)?;
                let value = map.next_value_seed(TagSeed)?;
                self.budget
                    .bytes(10 + value.len() + value.bytes().filter(|b| matches!(b, b'"' | b'\\')).count())
                    .map_err(de::Error::custom)?;
                Node::Tag(value)
            }
            Key::RuleId => {
                self.budget.leaf().map_err(de::Error::custom)?;
                let value: u64 = map.next_value()?;
                positive(value).map_err(de::Error::custom)?;
                self.budget.bytes(11 + digits(value)).map_err(de::Error::custom)?;
                Node::RuleId(value)
            }
            Key::RuleIdRange => {
                self.budget.leaf().map_err(de::Error::custom)?;
                let value = map.next_value::<WafRuleIdRange>()?;
                valid_range(value).map_err(de::Error::custom)?;
                self.budget
                    .bytes(33 + digits(value.start) + digits(value.end))
                    .map_err(de::Error::custom)?;
                Node::RuleIdRange(value)
            }
            Key::Phase => {
                self.budget.leaf().map_err(de::Error::custom)?;
                let value: u8 = map.next_value()?;
                valid_phase(value).map_err(de::Error::custom)?;
                self.budget.bytes(11).map_err(de::Error::custom)?;
                Node::Phase(value)
            }
            Key::AllOf | Key::AnyOf => {
                let is_all = matches!(key, Key::AllOf);
                self.budget.bytes(12).map_err(de::Error::custom)?;
                let children = map.next_value_seed(ChildrenSeed {
                    budget: self.budget,
                    depth: self.depth + 1,
                })?;
                if is_all {
                    Node::AllOf(children)
                } else {
                    Node::AnyOf(children)
                }
            }
            Key::Not => {
                self.budget.bytes(8).map_err(de::Error::custom)?;
                let mut children = Vec::new();
                // The child's depth/node guard runs before even the single slot is reserved.
                let child = map.next_value_seed(ExprSeed {
                    budget: self.budget,
                    depth: self.depth + 1,
                })?;
                children
                    .try_reserve_exact(1)
                    .map_err(|_| M::Error::custom(WafSelectionError::Allocation))?;
                children.push(child);
                Node::Not(children)
            }
            _ => return Err(invalid()),
        };
        if map.next_key::<de::IgnoredAny>()?.is_some() {
            return Err(invalid());
        }
        Ok(WafRuleSelectionExpr { node })
    }
}

struct ChildrenSeed<'a> {
    budget: &'a mut DecodeBudget,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for ChildrenSeed<'_> {
    type Value = Vec<WafRuleSelectionExpr>;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for ChildrenSeed<'_> {
    type Value = Vec<WafRuleSelectionExpr>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a nonempty WAF condition array")
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
        let mut children = Vec::new();
        // Ignore hostile size_hint; bound each child before descent/allocation.
        while let Some(child) = seq.next_element_seed(ExprSeed {
            budget: self.budget,
            depth: self.depth,
        })? {
            if !children.is_empty() {
                self.budget.bytes(1).map_err(de::Error::custom)?;
            }
            children
                .try_reserve_exact(1)
                .map_err(|_| S::Error::custom(WafSelectionError::Allocation))?;
            children.push(child);
        }
        if children.is_empty() {
            return Err(invalid());
        }
        Ok(children)
    }
}

struct TagSeed;
impl<'de> DeserializeSeed<'de> for TagSeed {
    type Value = String;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        // Preserve the resolved scalar type; YAML deserialize_str coerces numbers/bools/null.
        deserializer.deserialize_any(self)
    }
}
impl Visitor<'_> for TagSeed {
    type Value = String;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a bounded literal WAF tag")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        check_tag(value).map_err(E::custom)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(value.len())
            .map_err(|_| E::custom(WafSelectionError::Allocation))?;
        owned.push_str(value);
        Ok(owned)
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
        self.visit_str(&value)
    }
}

impl<'de> Deserialize<'de> for WafRuleSelectionExpr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ExprSeed {
            budget: &mut DecodeBudget::default(),
            depth: 1,
        }
        .deserialize(deserializer)
    }
}
impl<'de> Deserialize<'de> for WafRuleSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SelectionVisitor;
        impl<'de> Visitor<'de> for SelectionVisitor {
            type Value = WafRuleSelection;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a WAF selection object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut budget = DecodeBudget::default();
                budget.bytes(2).map_err(de::Error::custom)?;
                let mut selection = WafRuleSelection::default();
                let mut fields = 0;
                while let Some(key) = map.next_key::<Key>()? {
                    let slot = match key {
                        Key::Include => &mut selection.include,
                        Key::Exclude => &mut selection.exclude,
                        _ => return Err(invalid()),
                    };
                    if slot.is_some() {
                        return Err(invalid());
                    }
                    budget.bytes(10 + usize::from(fields != 0)).map_err(de::Error::custom)?;
                    *slot = Some(map.next_value_seed(ExprSeed {
                        budget: &mut budget,
                        depth: 1,
                    })?);
                    fields += 1;
                }
                selection.stats = selection.compute_stats().map_err(de::Error::custom)?;
                Ok(selection)
            }
        }
        deserializer.deserialize_map(SelectionVisitor)
    }
}

impl Serialize for WafRuleSelectionExpr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Node::Constant(value) = self.node {
            return serializer.serialize_bool(value);
        }
        let mut map = serializer.serialize_map(Some(1))?;
        match &self.node {
            Node::Tag(value) => map.serialize_entry("tag", value)?,
            Node::RuleId(value) => map.serialize_entry("ruleId", value)?,
            Node::RuleIdRange(value) => map.serialize_entry("ruleIdRange", value)?,
            Node::Phase(value) => map.serialize_entry("phase", value)?,
            Node::AllOf(value) => map.serialize_entry("allOf", value)?,
            Node::AnyOf(value) => map.serialize_entry("anyOf", value)?,
            Node::Not(value) => map.serialize_entry("not", &value[0])?,
            Node::Constant(_) => return Err(serde::ser::Error::custom("invalid WAF selection serialization state")),
        }
        map.end()
    }
}

// Schema-only shapes mirror the wire, never participate in decoding/evaluation.
#[derive(JsonSchema)]
#[serde(untagged)]
#[expect(dead_code, reason = "schema-only wire shape")]
enum ExprSchema {
    Boolean(bool),
    Operator(OperatorSchema),
}
#[derive(JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(dead_code, reason = "schema-only wire shape")]
enum OperatorSchema {
    Tag(String),
    RuleId(u64),
    RuleIdRange(WafRuleIdRange),
    Phase(u8),
    AllOf(Vec<WafRuleSelectionExpr>),
    AnyOf(Vec<WafRuleSelectionExpr>),
    Not(WafRuleSelectionExpr),
}
impl JsonSchema for WafRuleSelectionExpr {
    fn schema_name() -> Cow<'static, str> {
        "WafRuleSelectionExpr".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        ExprSchema::json_schema(generator)
    }
}

/// Configuration-only program, prepared once and reused for compilation trials.
/// Neither its instructions nor its metadata are retained by executable generations.
#[derive(Debug)]
pub struct PreparedWafRuleSelection {
    instructions: Vec<Instruction>,
    atoms: Vec<WafSelectionAtomRef>,
    include: Option<usize>,
    exclude: Option<usize>,
    owned_bytes: usize,
}

#[derive(Debug)]
enum Instruction {
    Constant(bool),
    Tag { value: String, atom: usize },
    RuleId { value: u64, atom: usize },
    RuleIdRange { value: WafRuleIdRange, atom: usize },
    Phase { value: u8, atom: usize },
    AllOf(usize),
    AnyOf(usize),
    Not,
}

/// Stable JSON pointer into the authored selection, without source-derived values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafSelectionAtomRef {
    pub pointer: String,
}

/// Fixed-size final membership summary. Unknown atoms are informational, not validation failures.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafSelectionSummary {
    pub raw_units: usize,
    pub source_active_units: usize,
    pub include_matches: usize,
    pub exclude_matches: usize,
    pub raw_include_matches: usize,
    pub raw_exclude_matches: usize,
    pub selected_units: usize,
    pub excluded_units: usize,
    pub unmatched_atoms: usize,
    pub empty_selection: bool,
}

pub(crate) struct SelectionDecision {
    pub mask: Vec<bool>,
    pub summary: WafSelectionSummary,
    pub raw_atom_matches: [usize; MAX_SELECTION_LEAVES],
    pub active_atom_matches: [usize; MAX_SELECTION_LEAVES],
}

fn owned_string(value: &str) -> Result<String, WafSelectionError> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| WafSelectionError::Allocation)?;
    result.push_str(value);
    Ok(result)
}

fn child_pointer(parent: &str, key: &str, index: Option<usize>) -> Result<String, WafSelectionError> {
    use std::fmt::Write;
    // At most 256 siblings; reserve the entire bounded numeric suffix before writing.
    let bytes = checked_sum(parent.len(), key.len() + 1 + if index.is_some() { 4 } else { 0 })?;
    let mut pointer = String::new();
    pointer
        .try_reserve_exact(bytes)
        .map_err(|_| WafSelectionError::Allocation)?;
    pointer.push_str(parent);
    pointer.push('/');
    pointer.push_str(key);
    if let Some(index) = index {
        write!(pointer, "/{index}").map_err(|_| WafSelectionError::Invalid)?;
    }
    Ok(pointer)
}

impl WafRuleSelection {
    /// Build a bounded postfix program from the already validated immutable wire tree.
    pub fn prepare(&self) -> Result<PreparedWafRuleSelection, WafSelectionError> {
        let mut prepared = PreparedWafRuleSelection {
            instructions: Vec::new(),
            atoms: Vec::new(),
            include: None,
            exclude: None,
            owned_bytes: size_of::<PreparedWafRuleSelection>(),
        };
        prepared
            .instructions
            .try_reserve_exact(self.stats.nodes)
            .map_err(|_| WafSelectionError::Allocation)?;
        prepared
            .atoms
            .try_reserve_exact(self.stats.metadata_leaves)
            .map_err(|_| WafSelectionError::Allocation)?;
        for (expression, pointer, include) in [(&self.include, "/include", true), (&self.exclude, "/exclude", false)] {
            if let Some(expression) = expression {
                let root = prepared.push_expression(expression, pointer)?;
                if include {
                    prepared.include = Some(root);
                } else {
                    prepared.exclude = Some(root);
                }
            }
        }
        prepared.owned_bytes = checked_sum(
            prepared.owned_bytes,
            prepared
                .instructions
                .capacity()
                .checked_mul(size_of::<Instruction>())
                .ok_or(WafSelectionError::Limit)?,
        )?;
        prepared.owned_bytes = checked_sum(
            prepared.owned_bytes,
            prepared
                .atoms
                .capacity()
                .checked_mul(size_of::<WafSelectionAtomRef>())
                .ok_or(WafSelectionError::Limit)?,
        )?;
        Ok(prepared)
    }
}

impl PreparedWafRuleSelection {
    pub(crate) fn instruction_count(&self) -> usize {
        self.instructions.len()
    }
    pub const fn owned_bytes(&self) -> usize {
        self.owned_bytes
    }
    pub fn atom_refs(&self) -> &[WafSelectionAtomRef] {
        &self.atoms
    }
    pub fn is_empty(&self) -> bool {
        self.instructions.is_empty()
    }

    /// Verify exact binding without allocating or preparing another program.
    pub fn matches_selection(&self, selection: &WafRuleSelection) -> bool {
        fn matches(expression: &WafRuleSelectionExpr, instructions: &[Instruction], cursor: &mut usize) -> bool {
            if let Node::AllOf(children) | Node::AnyOf(children) | Node::Not(children) = &expression.node {
                for child in children {
                    if !matches(child, instructions, cursor) {
                        return false;
                    }
                }
            }
            let Some(instruction) = instructions.get(*cursor) else {
                return false;
            };
            *cursor += 1;
            match (&expression.node, instruction) {
                (Node::Constant(a), Instruction::Constant(b)) => a == b,
                (Node::Tag(a), Instruction::Tag { value: b, .. }) => a == b,
                (Node::RuleId(a), Instruction::RuleId { value: b, .. }) => a == b,
                (Node::RuleIdRange(a), Instruction::RuleIdRange { value: b, .. }) => a == b,
                (Node::Phase(a), Instruction::Phase { value: b, .. }) => a == b,
                (Node::AllOf(a), Instruction::AllOf(b)) | (Node::AnyOf(a), Instruction::AnyOf(b)) => a.len() == *b,
                (Node::Not(_), Instruction::Not) => true,
                _ => false,
            }
        }
        let mut cursor = 0;
        for (expression, root) in [(&selection.include, self.include), (&selection.exclude, self.exclude)] {
            match expression {
                Some(expression) => {
                    if !matches(expression, &self.instructions, &mut cursor) || root != Some(cursor - 1) {
                        return false;
                    }
                }
                None => {
                    if root.is_some() {
                        return false;
                    }
                }
            }
        }
        cursor == self.instructions.len()
    }

    fn atom(&mut self, pointer: &str) -> Result<usize, WafSelectionError> {
        let pointer = owned_string(pointer)?;
        self.owned_bytes = checked_sum(self.owned_bytes, pointer.capacity())?;
        let index = self.atoms.len();
        self.atoms.push(WafSelectionAtomRef { pointer });
        Ok(index)
    }

    fn push_expression(&mut self, expression: &WafRuleSelectionExpr, path: &str) -> Result<usize, WafSelectionError> {
        let instruction = match &expression.node {
            Node::Constant(value) => Instruction::Constant(*value),
            Node::Tag(value) => {
                let value = owned_string(value)?;
                self.owned_bytes = checked_sum(self.owned_bytes, value.capacity())?;
                Instruction::Tag {
                    value,
                    atom: self.atom(&child_pointer(path, "tag", None)?)?,
                }
            }
            Node::RuleId(value) => Instruction::RuleId {
                value: *value,
                atom: self.atom(&child_pointer(path, "ruleId", None)?)?,
            },
            Node::RuleIdRange(value) => Instruction::RuleIdRange {
                value: *value,
                atom: self.atom(&child_pointer(path, "ruleIdRange", None)?)?,
            },
            Node::Phase(value) => Instruction::Phase {
                value: *value,
                atom: self.atom(&child_pointer(path, "phase", None)?)?,
            },
            Node::AllOf(children) | Node::AnyOf(children) | Node::Not(children) => {
                let key = match &expression.node {
                    Node::AllOf(_) => "allOf",
                    Node::AnyOf(_) => "anyOf",
                    _ => "not",
                };
                for (index, child) in children.iter().enumerate() {
                    let child_path = if key == "not" {
                        child_pointer(path, "not", None)?
                    } else {
                        child_pointer(path, key, Some(index))?
                    };
                    self.push_expression(child, &child_path)?;
                }
                match &expression.node {
                    Node::AllOf(_) => Instruction::AllOf(children.len()),
                    Node::AnyOf(_) => Instruction::AnyOf(children.len()),
                    _ => Instruction::Not,
                }
            }
        };
        let index = self.instructions.len();
        self.instructions.push(instruction);
        Ok(index)
    }

    fn evaluate(
        &self,
        unit: ValidatedRuleUnitView<'_, '_>,
        work: &mut CanonicalWork,
        limits: CompilerLimits,
        raw: &mut [usize; MAX_SELECTION_LEAVES],
        active: &mut [usize; MAX_SELECTION_LEAVES],
    ) -> Result<(bool, bool), CompilerError> {
        let mut stack = [false; MAX_SELECTION_NODES];
        let mut len = 0;
        let mut include = true;
        let mut exclude = false;
        for (index, instruction) in self.instructions.iter().enumerate() {
            work.charge_records(1, limits)?;
            let mut atom = None;
            let value = match instruction {
                Instruction::Constant(value) => *value,
                Instruction::Tag { value, atom: ordinal } => {
                    atom = Some(*ordinal);
                    let mut found = false;
                    // No short circuit: all compared metadata and members consume work budget.
                    for member in unit.members() {
                        work.charge_records(1, limits)?;
                        for action in member.actions() {
                            work.charge_records(1, limits)?;
                            if action.kind() == ActionEnvelopeKind::Action(ActionKind::Tag) {
                                if let ActionOperandView::Literal(tag) = action.operand() {
                                    work.charge_comparison_bytes(value.len().max(tag.text().len()), limits)?;
                                    found |= tag.text() == value;
                                }
                            }
                        }
                    }
                    found
                }
                Instruction::RuleId { value, atom: ordinal } => {
                    atom = Some(*ordinal);
                    unit.id() == *value
                }
                Instruction::RuleIdRange { value, atom: ordinal } => {
                    atom = Some(*ordinal);
                    (value.start..=value.end).contains(&unit.id())
                }
                Instruction::Phase { value, atom: ordinal } => {
                    atom = Some(*ordinal);
                    unit.root().resolved_state().phase() as u8 == *value
                }
                Instruction::AllOf(count) | Instruction::AnyOf(count) => {
                    let start = len - count;
                    let values = &stack[start..len];
                    let value = if matches!(instruction, Instruction::AllOf(_)) {
                        values.iter().all(|v| *v)
                    } else {
                        values.iter().any(|v| *v)
                    };
                    len = start;
                    value
                }
                Instruction::Not => {
                    len -= 1;
                    !stack[len]
                }
            };
            if let Some(atom) = atom {
                raw[atom] += usize::from(value);
                active[atom] += usize::from(value && unit.is_active());
            }
            stack[len] = value;
            len += 1;
            if self.include == Some(index) {
                include = value;
                len = 0;
            }
            if self.exclude == Some(index) {
                exclude = value;
                len = 0;
            }
        }
        Ok((include, exclude))
    }

    pub(crate) fn select(
        &self,
        document: &ValidatedSemanticDocument<'_>,
        work: &mut CanonicalWork,
        limits: CompilerLimits,
    ) -> Result<SelectionDecision, CompilerError> {
        let mut result = SelectionDecision {
            mask: Vec::new(),
            summary: WafSelectionSummary::default(),
            raw_atom_matches: [0; MAX_SELECTION_LEAVES],
            active_atom_matches: [0; MAX_SELECTION_LEAVES],
        };
        result
            .mask
            .try_reserve_exact(document.rule_units().len())
            .map_err(|_| CompilerError::Allocation {
                operation: "rule selection mask",
            })?;
        for unit in document.rule_units() {
            work.charge_records(1, limits)?;
            let (include, exclude) = self.evaluate(
                unit,
                work,
                limits,
                &mut result.raw_atom_matches,
                &mut result.active_atom_matches,
            )?;
            let selected = unit.is_active() && include && !exclude;
            result.mask.push(selected);
            result.summary.raw_units += 1;
            result.summary.raw_include_matches += usize::from(include);
            result.summary.raw_exclude_matches += usize::from(exclude);
            result.summary.source_active_units += usize::from(unit.is_active());
            result.summary.include_matches += usize::from(include && unit.is_active());
            result.summary.exclude_matches += usize::from(exclude && unit.is_active());
            result.summary.selected_units += usize::from(selected);
        }
        result.summary.excluded_units = result.summary.source_active_units - result.summary.selected_units;
        result.summary.unmatched_atoms = result.raw_atom_matches[..self.atoms.len()]
            .iter()
            .filter(|count| **count == 0)
            .count();
        result.summary.empty_selection = result.summary.selected_units == 0;
        Ok(result)
    }
}

/// Canonical literal tag projection over validated members of one complete rule unit.
pub fn rule_unit_tags<'view, 'source>(
    unit: ValidatedRuleUnitView<'view, 'source>,
) -> impl Iterator<Item = &'source str> + 'view
where
    'source: 'view,
{
    unit.members().flat_map(|member| member.actions()).filter_map(|action| {
        if action.kind() == ActionEnvelopeKind::Action(ActionKind::Tag) {
            if let ActionOperandView::Literal(tag) = action.operand() {
                return Some(tag.text());
            }
        }
        None
    })
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/tag_selection_test.rs"]
mod tests;
