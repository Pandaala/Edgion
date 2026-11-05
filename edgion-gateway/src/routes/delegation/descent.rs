//! Request-time cascading match through delegation slots.
//!
//! Everything here is reached only after a rule unit that holds a delegation
//! slot has matched its own conditions. Ordinary candidates never touch this
//! module: no state is created, no slot is loaded, no mask is checked.

use super::{DelegationSlot, InheritedRuleValues};
use crate::ctx::EdgionHttpContext;
use crate::plugins::runtime::PluginRuntime;
use crate::routes::grpc::GrpcRouteRuleUnit;
use crate::routes::http::routes_mgr::RouteRules;
use crate::routes::http::HttpRouteRuleUnit;
use edgion_resources::err::EdError;
use edgion_resources::resources::route_delegation::MAX_ROUTE_DELEGATION_LEVELS;
use pingora_proxy::Session;
use smallvec::SmallVec;
use std::sync::Arc;

/// Maximum number of successful slot entries one request may make, summed over
/// every hostname group and every backtracking alternative. Once exhausted,
/// every further delegating candidate is a miss. This bounds the per-request
/// descent work (and the linear `visited` scan) independently of how many
/// delegating rules tenants attach below a shared parent.
pub const MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST: usize = 64;

/// Per-request descent bookkeeping, created lazily on the first delegating hit.
///
/// Shared by every descent of one request (all hostname groups and all
/// backtracking alternatives), so a slot is entered at most once per request.
pub struct DescentState {
    /// Slots already entered by this request (including those on the current
    /// chain). First entry wins: a later candidate pointing at a visited slot is
    /// a miss. Keyed by slot address only. Its length is the number of
    /// successful entries, so it never exceeds
    /// [`MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST`] and the linear scan stays
    /// bounded.
    visited: SmallVec<[usize; 8]>,
    /// OR of the standard-filter masks of the delegating levels on the current chain.
    chain_mask: u32,
    /// Length of the current chain, counting the root level.
    depth: u8,
}

impl Default for DescentState {
    fn default() -> Self {
        Self::new()
    }
}

impl DescentState {
    /// A state positioned at the root level (chain length 1).
    pub fn new() -> Self {
        Self {
            visited: SmallVec::new(),
            chain_mask: 0,
            depth: 1,
        }
    }

    /// Whether a candidate with `unit_mask` conflicts with a mandatory filter
    /// already on the chain.
    #[inline]
    pub fn conflicts(&self, unit_mask: u32) -> bool {
        self.chain_mask & unit_mask != 0
    }

    /// Whether a descent into the slot at `ptr` is admitted: the request still
    /// has entry budget, the slot was not entered before, the chain is not
    /// full, and the unit's own filters do not conflict with the chain.
    #[inline]
    fn admits(&self, ptr: usize, unit_mask: u32) -> bool {
        self.visited.len() < MAX_DELEGATION_SLOT_ENTRIES_PER_REQUEST
            && (self.depth as usize) < MAX_ROUTE_DELEGATION_LEVELS
            && !self.conflicts(unit_mask)
            && !self.visited.contains(&ptr)
    }

    /// Record a successful entry: mark the slot visited (consuming one unit of
    /// the request's entry budget), join the unit's mask, grow the chain.
    #[inline]
    fn commit(&mut self, ptr: usize, unit_mask: u32) {
        self.visited.push(ptr);
        self.chain_mask |= unit_mask;
        self.depth += 1;
    }

    #[inline]
    fn slot_ptr<S>(slot: &Arc<DelegationSlot<S>>) -> usize {
        Arc::as_ptr(slot) as *const () as usize
    }

    /// Try to descend from a matched delegating unit into `slot`.
    ///
    /// Returns `None` (the delegating candidate is a miss) when the request's
    /// entry budget is exhausted, the slot was already entered by this request,
    /// the chain is full, or the unit's own filters conflict with the chain. An
    /// empty slot is also a miss, and it neither consumes entry budget nor
    /// marks the slot visited. On success the slot is marked visited, the
    /// unit's mask joins the chain mask, the chain grows by one level, and the
    /// loaded subtree (one generation for the descent) is returned.
    pub fn enter_loaded<S>(&mut self, slot: &Arc<DelegationSlot<S>>, unit_mask: u32) -> Option<Arc<S>> {
        let ptr = Self::slot_ptr(slot);
        if !self.admits(ptr, unit_mask) {
            return None;
        }
        let subtree = slot.load()?;
        self.commit(ptr, unit_mask);
        Some(subtree)
    }

    /// Chain position to restore when a subtree yields nothing.
    #[inline]
    pub fn mark(&self) -> (u32, u8) {
        (self.chain_mask, self.depth)
    }

    /// Pop back to `mark` after a subtree miss. The visited set is kept.
    #[inline]
    pub fn leave(&mut self, mark: (u32, u8)) {
        (self.chain_mask, self.depth) = mark;
    }
}

/// Resolved per-level route ExtensionRef targets, aligned with the level's
/// `plugin_runtime.ext_ref_keys()`.
pub type LevelExtRefs = Arc<[Option<Arc<PluginRuntime>>]>;

/// Ancestors of a delegated match, root first. The leaf is the result's
/// `route_unit` (HTTP) or `grpc_route_unit` (gRPC); ordinary matches carry no
/// `DelegatedMatch` at all.
///
/// A chain is single-kind: an HTTPRoute chain fills `ancestors`, a GRPCRoute
/// chain fills `grpc_ancestors`, and the other list stays empty. Two parallel
/// lists keep every HTTP consumer of `ancestors` unchanged; per-level indexes
/// (`ext_refs`, plugin levels) align with whichever list is filled.
pub struct DelegatedMatch {
    pub ancestors: SmallVec<[Arc<HttpRouteRuleUnit>; 4]>,
    /// GRPCRoute ancestors, root first (empty for an HTTPRoute chain).
    pub grpc_ancestors: SmallVec<[Arc<GrpcRouteRuleUnit>; 4]>,
    /// Ancestor fallbacks for the leaf's unset timeouts/retry/sessionPersistence,
    /// folded once when the match is built.
    pub inherited: InheritedRuleValues,
    /// Each ancestor's resolved ExtensionRef targets (aligned with `ancestors`),
    /// installed after the root-matched Gateway's plugin scope is captured.
    /// Empty until then; `None` for an ancestor without ExtensionRefs.
    pub ext_refs: SmallVec<[Option<LevelExtRefs>; 4]>,
}

impl DelegatedMatch {
    /// Build the chain record and fold the inherited rule values once.
    pub fn new(ancestors: SmallVec<[Arc<HttpRouteRuleUnit>; 4]>) -> Self {
        let inherited = InheritedRuleValues::fold(&ancestors);
        Self {
            ancestors,
            grpc_ancestors: SmallVec::new(),
            inherited,
            ext_refs: SmallVec::new(),
        }
    }

    /// Build a GRPCRoute chain record and fold its inherited rule values once.
    pub fn new_grpc(grpc_ancestors: SmallVec<[Arc<GrpcRouteRuleUnit>; 4]>) -> Self {
        let inherited = InheritedRuleValues::fold_grpc(&grpc_ancestors);
        Self {
            ancestors: SmallVec::new(),
            grpc_ancestors,
            inherited,
            ext_refs: SmallVec::new(),
        }
    }

    /// Number of ancestors of either kind (only one list is ever filled).
    #[inline]
    pub fn len(&self) -> usize {
        self.ancestors.len() + self.grpc_ancestors.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Rule-level plugin runtime of ancestor `level`, whichever kind the chain is.
    #[inline]
    pub fn level_runtime(&self, level: usize) -> Option<&Arc<PluginRuntime>> {
        match self.ancestors.get(level) {
            Some(unit) => Some(&unit.plugin_runtime),
            None => self.grpc_ancestors.get(level).map(|unit| &unit.plugin_runtime),
        }
    }

    /// ExtensionRef keys of every ancestor, root first, whichever kind.
    pub(crate) fn level_ext_ref_keys(&self) -> impl Iterator<Item = &[Option<String>]> {
        self.ancestors
            .iter()
            .map(|unit| unit.plugin_runtime.ext_ref_keys())
            .chain(
                self.grpc_ancestors
                    .iter()
                    .map(|unit| unit.plugin_runtime.ext_ref_keys()),
            )
    }

    /// ExtensionRef target `index` of ancestor `level`.
    #[inline]
    pub fn ext_ref(&self, level: usize, index: usize) -> Option<Arc<PluginRuntime>> {
        self.ext_refs.get(level)?.as_ref()?.get(index)?.as_ref().map(Arc::clone)
    }
}

/// A terminal match found inside a subtree. `U` is the route kind's rule unit.
pub(crate) struct ChildHit<U = HttpRouteRuleUnit> {
    pub(crate) leaf: Arc<U>,
    /// Delegating units between the subtree entry and the leaf, top first.
    pub(crate) ancestors: SmallVec<[Arc<U>; 4]>,
}

/// Kind-independent descent from a matched delegating `unit` into `slot`.
///
/// Loads the subtree once and runs `match_subtree` on it; an empty slot is a
/// miss that costs no entry budget. On a hit the unit is
/// put at the front of the hit's ancestors; on a miss the chain position is
/// restored so the caller can backtrack.
#[inline]
pub(crate) fn descend_with<U, S>(
    unit: &Arc<U>,
    unit_mask: u32,
    slot: &Arc<DelegationSlot<S>>,
    state: &mut DescentState,
    match_subtree: impl FnOnce(&S, &mut DescentState) -> Result<Option<ChildHit<U>>, EdError>,
) -> Result<Option<ChildHit<U>>, EdError> {
    let mark = state.mark();
    let Some(subtree) = state.enter_loaded(slot, unit_mask) else {
        return Ok(None);
    };
    let hit = match_subtree(&subtree, state)?;
    match hit {
        Some(mut hit) => {
            hit.ancestors.insert(0, Arc::clone(unit));
            Ok(Some(hit))
        }
        None => {
            state.leave(mark);
            Ok(None)
        }
    }
}

/// Descend from a matched delegating `unit` into `slot`.
///
/// The subtree is loaded once, so the whole descent (and the returned `Arc`s)
/// belongs to one generation even if the slot is swapped meanwhile. A miss
/// restores the chain position so the caller can backtrack.
pub(crate) fn descend(
    unit: &Arc<HttpRouteRuleUnit>,
    slot: &Arc<DelegationSlot<RouteRules>>,
    session: &Session,
    ctx: &EdgionHttpContext,
    state: &mut DescentState,
) -> Result<Option<ChildHit>, EdError> {
    descend_with(unit, unit.filter_mask, slot, state, |subtree, state| {
        subtree.match_child(session, ctx, state)
    })
}

/// Evaluate one child-level candidate whose path already matched.
///
/// Checks the chain mask, then the unit's own method/header/query conditions.
/// A delegating child descends recursively (bounded by the chain depth).
#[inline]
pub(crate) fn match_child_unit(
    unit: &Arc<HttpRouteRuleUnit>,
    session: &Session,
    ctx: &EdgionHttpContext,
    state: &mut DescentState,
) -> Result<Option<ChildHit>, EdError> {
    if state.conflicts(unit.filter_mask) || !unit.deep_match_child(session, ctx)? {
        return Ok(None);
    }
    match &unit.delegate {
        None => Ok(Some(ChildHit {
            leaf: Arc::clone(unit),
            ancestors: SmallVec::new(),
        })),
        Some(slot) => descend(unit, slot, session, ctx, state),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/delegation/descent_test.rs"]
mod tests;
