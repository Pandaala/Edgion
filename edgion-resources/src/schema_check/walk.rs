//! The schema walker itself. See `mod.rs` for the public entry point.
//!
//! Rules (mirrors `01-design.md` §3 / `05-issues.md` R-4, R-5 verbatim):
//!
//! 1. Resolve `$ref` against the schema's `$defs`; merge `allOf` branches
//!    into one allowed-property set. `#[serde(flatten)]` needs NO special
//!    handling — schemars inlines flattened fields directly into
//!    `properties` (measured; see `01-design.md` §3).
//!
//!    JSON Schema 2020-12 permits keywords beside `$ref`, and schemars 1.2.1
//!    uses exactly that for an INTERNALLY-tagged enum branch
//!    (`#[serde(tag = "type")]`, no `content`): the branch node itself
//!    carries `{"$ref": "#/$defs/Variant", "properties": {"type": {"const":
//!    "..."}}, ...}` — the discriminator lives in the node's OWN sibling
//!    keywords, while the variant's real fields live in the `$ref` target.
//!    So resolution here MERGES a node with everything its `$ref` chain
//!    points to (own keywords take priority on key collision, though none
//!    are observed in this codebase); it never discards the referencing
//!    node's own keywords the way a naive "replace with target" resolve
//!    would (that bug produced a false-positive report of the discriminator
//!    key itself, since the merge that carries it was skipped).
//! 2. Objects: allowed = the union of `properties` keys (merged across the
//!    `$ref` chain per rule 1). Report an input key only when the node
//!    declares `properties` (anywhere in the chain), emits NO
//!    `additionalProperties` keyword (anywhere in the chain) that isn't
//!    `false`, and the key is not in the allowed set.
//!    `additionalProperties` present (schema or `true`, anywhere in the
//!    chain) means a free-form map: stay silent for that whole node. No
//!    `properties` anywhere in the chain: silent.
//! 3. `oneOf` / `anyOf`: pick the branch whose `const` discriminator matches
//!    the input's value for that same key — checked against the branch's
//!    OWN merged (chain-resolved) properties, so an internally-tagged
//!    branch's `$ref`-sibling discriminator is visible. If no branch
//!    matches, report only the keys unknown in EVERY branch and do not
//!    recurse — never guess.
//! 4. Arrays: walk `items` per element, appending `[i]` to the path.
//! 5. `Option<T>` yields a nullable union; a `null` input is a leaf (silent).
//! 6. Bounds: cap recursion depth and total visited nodes, reusing
//!    `MAX_JSON_INSTANCE_DEPTH` / `MAX_JSON_INSTANCE_NODES` rather than
//!    inventing new constants. Every examined key counts against the node
//!    bound, including unknown ones that are reported without recursion —
//!    counting only recursed nodes left the sibling-width case unbounded.
//!
//! Known blind spot: a field-level `#[serde(alias = "...")]` would be accepted
//! by serde but is absent from the schema, so the alias would be reported as
//! unknown. No struct field in `edgion-resources` uses one today (the only
//! `alias` attributes are on enum VARIANT names, which this walker never
//! inspects). Adding a field-level alias to a plugin config would require
//! teaching the walker about it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

// Reused rather than invented: these already bound the depth/width of a
// plugin config JSON instance elsewhere in this crate (subtask spec item 8).
use crate::resources::edgion_plugins::plugin_configs::{MAX_JSON_INSTANCE_DEPTH, MAX_JSON_INSTANCE_NODES};

/// Max `$ref` hops resolved in a single lookup. Every `$ref` chain measured
/// in this codebase is exactly one hop (see `edgion-resources/tests/crd_coverage.rs`);
/// this only guards against a future schema generator emitting a longer chain.
const MAX_REF_HOPS: usize = 8;

/// Follow a node's `$ref` chain (bounded), returning every node visited in
/// order: the node itself first, then each successive `$ref` target. See the
/// module doc for why this must be a chain of nodes to merge, not a single
/// "final" node to replace with.
fn ref_chain<'a>(node: &'a Value, defs: &'a Map<String, Value>) -> Vec<&'a Value> {
    let mut chain = vec![node];
    let mut current = node;
    for _ in 0..MAX_REF_HOPS {
        let Some(name) = current
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
        else {
            break;
        };
        match defs.get(name) {
            Some(target) => {
                chain.push(target);
                current = target;
            }
            None => break,
        }
    }
    chain
}

/// Union of `properties` across every node in the chain. A node's own
/// keyword wins on key collision (`or_insert`, chain is self-first); no
/// collision is observed in this codebase (an internally-tagged branch's own
/// keys are only ever the tag itself, never a variant field name).
fn chain_properties<'a>(chain: &[&'a Value]) -> BTreeMap<&'a str, &'a Value> {
    let mut props = BTreeMap::new();
    for node in chain {
        if let Some(p) = node.get("properties").and_then(Value::as_object) {
            for (k, v) in p {
                props.entry(k.as_str()).or_insert(v);
            }
        }
    }
    props
}

/// Whether any node in the chain literally declares a `properties` keyword
/// (even an empty `{}`, e.g. a zero-field plugin config struct) — distinct
/// from `chain_properties` being non-empty, which an empty-but-present
/// `properties: {}` would miss.
fn chain_has_properties_keyword(chain: &[&Value]) -> bool {
    chain
        .iter()
        .any(|n| n.get("properties").and_then(Value::as_object).is_some())
}

/// `additionalProperties` present (as a schema or `true`) anywhere in the
/// chain => free-form map. `false` alone does not count — see rule 2 /
/// `05-issues.md` R-4.
fn chain_is_free_form(chain: &[&Value]) -> bool {
    chain.iter().any(|n| {
        n.get("additionalProperties")
            .is_some_and(|ap| !matches!(ap, Value::Bool(false)))
    })
}

/// Concatenate every `key`-named array across the chain (e.g. `oneOf`
/// entries). Not observed split across chain nodes in this codebase; this
/// just makes the chain-aware lookup total instead of "first node wins".
fn chain_array<'a>(chain: &[&'a Value], key: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    for node in chain {
        if let Some(arr) = node.get(key).and_then(Value::as_array) {
            out.extend(arr.iter());
        }
    }
    out
}

/// First `key` found scanning the chain in order.
fn chain_first<'a>(chain: &[&'a Value], key: &str) -> Option<&'a Value> {
    chain.iter().find_map(|n| n.get(key))
}

/// Whether the chain marks this node as JSON-Schema `"type": "null"` (how
/// `Option<T>` nullability renders as an `anyOf` branch).
fn chain_is_null_type(chain: &[&Value]) -> bool {
    chain
        .iter()
        .any(|n| n.get("type").and_then(Value::as_str) == Some("null"))
}

/// The effective shape of an object-typed schema node, resolved against one
/// concrete input object (needed for `oneOf`/`anyOf` discriminator dispatch,
/// which is data-dependent).
struct ObjectView<'a> {
    /// Known key -> sub-schema to recurse into. `None` means the key is
    /// known (so must not be reported) but its schema is ambiguous — an
    /// unresolved `oneOf`/`anyOf` dispatch — so it must not be recursed into
    /// either (rule 3: never guess).
    known: BTreeMap<&'a str, Option<&'a Value>>,
    /// `additionalProperties` is present as a schema or `true` anywhere in
    /// the node's `$ref` chain: a free-form map. This is a DELIBERATE
    /// deviation from literal JSON-Schema semantics, where an ABSENT
    /// `additionalProperties` means "extras allowed" — schemars 1.2.1 emits
    /// no `additionalProperties` keyword at all for an ordinary Rust struct
    /// (measured), so this walker treats "properties present,
    /// additionalProperties absent" as the closed set that a Rust struct
    /// actually is, and treats an explicit `additionalProperties` (schema or
    /// `true`, e.g. a `HashMap<String, Value>` field) as the free-form
    /// escape hatch. `additionalProperties: false` (schemars' rendering of
    /// `#[serde(deny_unknown_fields)]`) is NOT free-form; it falls through to
    /// the ordinary closed-set path below. See `05-issues.md` R-4.
    free_form: bool,
    /// This node describes an object at all (declares `properties` itself,
    /// or a contributing `allOf`/`oneOf`/`anyOf` branch does). If false,
    /// there is nothing to compare against and the node is silent.
    is_object: bool,
}

impl<'a> ObjectView<'a> {
    fn empty() -> Self {
        ObjectView {
            known: BTreeMap::new(),
            free_form: false,
            is_object: false,
        }
    }
}

/// Resolve the effective object schema at `node` against `value_obj`,
/// merging the `$ref` chain (rule 1), `allOf` branches, and dispatching
/// `oneOf`/`anyOf` by `const` discriminator (rule 3).
fn object_view<'a>(node: &'a Value, value_obj: &Map<String, Value>, defs: &'a Map<String, Value>) -> ObjectView<'a> {
    let chain = ref_chain(node, defs);
    let mut view = ObjectView::empty();

    // A boolean schema node (`true`/`false`) is schemars' rendering of an
    // unconstrained `serde_json::Value` field (e.g. `WasmRawJson.json`) —
    // "accept anything" or "accept nothing", never a named-key object to
    // compare against. Treat it as free-form so the caller stays silent.
    if chain.iter().any(|n| n.is_boolean()) {
        view.free_form = true;
        view.is_object = true;
        return view;
    }

    if chain_is_free_form(&chain) {
        view.free_form = true;
        view.is_object = true;
        return view;
    }

    if chain_has_properties_keyword(&chain) {
        view.is_object = true;
        for (k, sub) in chain_properties(&chain) {
            view.known.insert(k, Some(sub));
        }
    }

    // allOf: merge every branch's allowed keys into one set (rule 1). Not
    // observed in this codebase's schemars 1.2.1 output (no allOf is ever
    // emitted), so this path is defensive/unverified against real schemas —
    // implemented per spec in case a future schemars version or a manual
    // `#[schemars(extend = ...)]` introduces one.
    for branch in chain_array(&chain, "allOf") {
        merge_view(&mut view, object_view(branch, value_obj, defs));
        if view.free_form {
            return view;
        }
    }

    for key in ["oneOf", "anyOf"] {
        let branches = chain_array(&chain, key);
        if !branches.is_empty() {
            apply_tagged_branches(&mut view, &branches, value_obj, defs);
        }
    }

    view
}

/// Fold `sub` into `view`, keeping whatever `view` already knows about a key
/// (base object properties, or an earlier branch, take precedence).
fn merge_view<'a>(view: &mut ObjectView<'a>, sub: ObjectView<'a>) {
    if sub.free_form {
        view.free_form = true;
        view.is_object = true;
        return;
    }
    view.is_object |= sub.is_object;
    for (k, schema) in sub.known {
        view.known.entry(k).or_insert(schema);
    }
}

/// Handle one `oneOf`/`anyOf` branch list against `value_obj` (rule 3 / rule
/// 5). `branches` are the raw (not yet `$ref`-resolved) entries; each is
/// resolved (as a chain, per rule 1) internally.
///
/// - Pure-null branches (`{"type": "null"}`, how `Option<T>` renders
///   nullability) contribute no keys and are dropped first.
/// - If exactly one non-null branch remains, it is unambiguous (this is how
///   `Option<T>` for an object `T` resolves: `anyOf: [{$ref: T}, {type:
///   null}]`) — recurse into it directly.
/// - Otherwise, try to find the unique branch whose chain-merged
///   `properties` contains a `const` matching the input's value at that same
///   key (the `#[serde(tag = "type", content = "config")]` /
///   `#[serde(tag = "type")]` discriminator pattern — for an
///   internally-tagged branch the discriminator lives in the branch node's
///   OWN sibling `properties`, not its `$ref` target, which is exactly why
///   this must read the chain-merged view rather than the raw branch node).
/// - If no unique branch is identified, never guess: union every candidate
///   branch's keys (so a key legal in ANY branch is not reported) but mark
///   them all non-recursable — only keys unknown in EVERY branch surface,
///   and nothing beneath them is inspected.
fn apply_tagged_branches<'a>(
    view: &mut ObjectView<'a>,
    branches: &[&'a Value],
    value_obj: &Map<String, Value>,
    defs: &'a Map<String, Value>,
) {
    let candidates: Vec<&'a Value> = branches
        .iter()
        .copied()
        .filter(|b| !chain_is_null_type(&ref_chain(b, defs)))
        .collect();

    if candidates.is_empty() {
        return;
    }

    if let [only] = candidates.as_slice() {
        merge_view(view, object_view(only, value_obj, defs));
        return;
    }

    let mut matched: Option<&'a Value> = None;
    let mut match_count = 0u32;
    for branch in &candidates {
        let branch_chain = ref_chain(branch, defs);
        let props = chain_properties(&branch_chain);
        let branch_matches = props
            .iter()
            .any(|(k, sub)| sub.get("const").is_some_and(|c| value_obj.get(*k) == Some(c)));
        if branch_matches {
            matched = Some(branch);
            match_count += 1;
        }
    }

    if match_count == 1 {
        merge_view(view, object_view(matched.expect("match_count == 1"), value_obj, defs));
        return;
    }

    // Ambiguous: union candidate keys, but never recurse into any of them.
    for branch in &candidates {
        let sub = object_view(branch, value_obj, defs);
        if sub.free_form {
            view.free_form = true;
            view.is_object = true;
            return;
        }
        view.is_object |= sub.is_object;
        for (k, _) in sub.known {
            view.known.entry(k).or_insert(None);
        }
    }
}

/// Resolve the `items` schema for an array-typed node, including through a
/// wrapping `oneOf`/`anyOf` (e.g. `Option<Vec<T>>`). `None` when no `items`
/// schema can be found unambiguously — silent (rule 2's "nothing to compare"
/// extended to arrays).
fn array_items<'a>(node: &'a Value, defs: &'a Map<String, Value>) -> Option<&'a Value> {
    let chain = ref_chain(node, defs);
    if let Some(items) = chain_first(&chain, "items") {
        return Some(items);
    }
    for key in ["oneOf", "anyOf"] {
        let branches = chain_array(&chain, key);
        let mut found: Option<&'a Value> = None;
        let mut count = 0u32;
        for b in &branches {
            let b_chain = ref_chain(b, defs);
            if let Some(items) = chain_first(&b_chain, "items") {
                found = Some(items);
                count += 1;
            }
        }
        if count == 1 {
            return found;
        }
    }
    None
}

fn child_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// Walk `value` against schema `node`, appending unknown-key paths to
/// `findings`. `depth` and `node_count` enforce the caps in rule 6; both are
/// checked on every call so a pathological document (very deep, or very
/// wide via siblings/array elements) terminates cleanly instead of
/// panicking or looping unbounded (subtask spec item 8 / test 9).
pub(super) fn walk(
    value: &Value,
    node: &Value,
    defs: &Map<String, Value>,
    path: &str,
    depth: usize,
    node_count: &mut u64,
    findings: &mut Vec<String>,
) {
    if depth > MAX_JSON_INSTANCE_DEPTH {
        return;
    }
    *node_count += 1;
    if *node_count > MAX_JSON_INSTANCE_NODES {
        return;
    }

    // A boolean schema is "accept anything" / "accept nothing" — never a set
    // of named keys. Stay silent regardless of the value's JSON type. (A
    // `$ref`-chain boolean is caught inside `object_view`; this is only the
    // cheap top-level shortcut for a bare `true`/`false` node.)
    if node.is_boolean() {
        return;
    }

    match value {
        // Also how `Option<T>` nullability resolves at the leaf (rule 5):
        // a `null` input never has keys to check.
        Value::Null => {}
        Value::Object(map) => {
            let view = object_view(node, map, defs);
            if view.free_form || !view.is_object {
                return;
            }
            for (key, child_value) in map {
                // Count EVERY key examined, not only the ones recursed into. An
                // unknown key never calls `walk`, so counting inside `walk`
                // alone left this loop unbounded for the single shape this
                // feature exists to catch — a flat config block full of
                // unrecognized keys. A document with N unknown siblings then
                // produced N findings regardless of the cap, and those findings
                // live on `spec.unknown_fields` for the resource's lifetime.
                *node_count += 1;
                if *node_count > MAX_JSON_INSTANCE_NODES {
                    return;
                }
                match view.known.get(key.as_str()) {
                    Some(Some(child_schema)) => {
                        let path = child_path(path, key);
                        walk(child_value, child_schema, defs, &path, depth + 1, node_count, findings);
                    }
                    // Known via an ambiguous oneOf/anyOf dispatch: reported
                    // neither here nor below it (rule 3: never guess).
                    Some(None) => {}
                    None => findings.push(child_path(path, key)),
                }
                if *node_count > MAX_JSON_INSTANCE_NODES {
                    return;
                }
            }
        }
        Value::Array(items) => {
            let Some(item_schema) = array_items(node, defs) else {
                return;
            };
            for (i, item) in items.iter().enumerate() {
                let path = format!("{path}[{i}]");
                walk(item, item_schema, defs, &path, depth + 1, node_count, findings);
                if *node_count > MAX_JSON_INSTANCE_NODES {
                    return;
                }
            }
        }
        // Scalars are leaves: nothing to compare.
        Value::String(_) | Value::Number(_) | Value::Bool(_) => {}
    }
}
