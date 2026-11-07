---
name: tx-unit-staging-in-place
description: Use when reviewing native WAF rule-unit staging, TX setvar/ctl rollback, chain frontier copies, or TX byte accounting during a unit.
knowledge_type: regression-guards
---

# Rule units stage TX edits in place with an undo journal

## Conclusion and scope

`Transaction::evaluate_unit_into` (`edgion-waf/src/seclang/transaction/units.rs`)
no longer copies the TX map when a unit's first member matches. The base
`TxState` is moved out of `self.tx` into the unit, and a default placeholder
that carries only the base `retained_bytes` stands in for it. `TxState::begin_unit`
(`transaction/storage.rs`) moves the prior-unit match observers
(`MATCHED_VAR`, `MATCHED_VAR_NAME`, `MATCHED_VARS`) and every scalar that a
rollback restores into a `TxCheckpoint`. The unit then starts in the same state
the former per-unit copy had.

- **Single-member units** edit the staged state directly. `set`, `delete`,
  `add_removed_unit` and `add_target_exclusion` take an optional `TxJournal`
  and reserve every journal slot before editing. Entry edits are collapsed per
  entry. The first replacement of a base entry saves its base value
  (`TxUndo::SavedValue`). Removing a base entry records `TxUndo::RemovedEntry`
  with its original key and value; if the value was already replaced, the saved
  base value is moved in and the staged value is dropped. Every later displaced
  value, and every value of an entry the unit appended, is dropped exactly as a
  plain `set`/`delete` drops it. Surviving base entries keep their relative
  order and precede every appended entry, so while the unit has recorded fewer
  than `SCAN_LIMIT` (16) steps, an entry's base origin and saved value are
  derived by scanning the steps, including the removed origins. A per-entry
  `EntryMark` vector parallel to `entries` (base origin plus saved-step index)
  is built only when the unit records more steps. Later actions in the same unit
  (`+=`/`-=`, macro expansion, `&TX:` counts, capture replacement) read the
  staged values directly.
- **Commit** (`admit_unit_commit` passes) publishes the staged state as
  `self.tx` and clears the journal, which keeps its capacity.
- **No match or any error**, including `TxVariableLimit`, `TxByteLimit`,
  `CtlEntryLimit`, `SetVarActionLimit`, a runtime-action validation failure and
  the commit `RetainedByteLimit`, runs `rollback_unit`. It writes saved values
  back in place, drops the entries the unit appended (surviving base entries
  keep their relative order), reinserts removed originals at their base indices
  in ascending order, undoes the ctl-list insertions in reverse order and
  restores the checkpoint, including the cached `retained_bytes`. The base is
  then content-identical and byte-identical (`entries` order, payload counters
  and observers). Vectors never shrink during a unit, so a rollback does not
  allocate.
- **Chain units** keep the frontier model. The in-place staged state is the
  single seed branch: it is only read and copied (`TxState::try_clone`) into
  children and charged as one `ChainBranch` (`staged_branch_bytes`). It is never
  edited, so its journal stays empty. A winning branch copy replaces the base.
  Branch copies are the only remaining whole-TX copies.

The journal (`Transaction::tx_journal`) is two per-transaction vectors (marks
and steps) that grow geometrically and are cleared, never reallocated, between
units. It is not
a pool, thread-local or lock.

## Rationale and evidence

The 2026-11-07 profile (`docs/superpowers/plans/2026-11-07-waf-second-pass-baseline.md`,
hotspot H2) measured the former `try_clone_for_unit` copy at 10.9 % of clean_get
CPU. Dropping the old map on commit added 7.3 %. The copy was the largest
allocation source on header-only requests: CRS runs about 60 matching
initialization and skip units per request, and each one paid a full copy.

Accounting stays identical by construction:

- During the unit, `self.tx.retained_bytes` is still the base charge.
- `match_storage` and selection still add the staged growth over the base.
- Admission still charges `staged.retained_bytes`.
- Chain frontier bytes are unchanged, because `size_of::<TxState>()` and
  `size_of::<ChainBranch>()` are unchanged. The journal is a `Transaction`
  field, not a `TxState` field.

The journal itself is not charged. Its bound is structural: it saves at most
one original per base entry, so its payload never exceeds the base TX payload
at unit start, which is within `max_tx_bytes`. Its marks, when built, hold
two `u32` indices per entry, bounded by `max_tx_variables`, and its ctl steps
hold one index per ctl insertion. An earlier form of this journal kept every displaced value. A
multi-match unit with a self-appending setvar then retained the sum of all
intermediate values (2 128 384 bytes for 64 one-KiB matches over a 1 067-byte
base) before `TxByteLimit` could fire. With per-entry collapsing the same unit
saves 1 000 bytes.

## Alternatives and limits

- **A lookup overlay that leaves the base untouched** was rejected. Every TX
  reader (`SelectedBytes::Tx(index)`, collection selection, macros, counts, ctl
  checks) would need a merged view with stable indices.
- **Copy-on-write shared bytes for `MatchValue`** was rejected. It would keep a
  per-unit vector copy and add reference counts to every value.
- **Not converted to the journal:** chain branches still copy the staged state
  once per first-member match, and `capture_keys` is still copied per matched
  unit. That copy does not allocate when it is empty.

## Verification and re-evaluation

Verified on 2026-11-07 with:

- `cargo test -p edgion-waf`. The new test
  `in_place_unit_staging_matches_frozen_copy_on_commit_rollback_and_every_limit`
  compares `evaluate_unit` with the frozen copy-based oracle
  (`frozen_clone_unit_into`). It checks results, the full TX snapshot,
  `retained_bytes()`, scratch and work counters, and that a rollback restores
  the base exactly. Single-member and chain units are swept across
  `max_tx_variables`, `max_tx_bytes`, `max_ctl_entries`,
  `max_setvar_actions` and `max_retained_bytes` thresholds. The sources include
  repeated sets and deletes of one key inside a unit, delete-after-set-after-set
  of base entries and self-appending matches.
- `unit_journal_keeps_at_most_the_base_value_of_each_entry` (2026-11-08) checks
  through the `TxJournalPayloadBytes` hook that the journal stays within the base
  TX on commit and on a mid-unit `TxByteLimit` rollback.
- `matched_single_member_units_stage_in_place_without_copying_tx` checks for
  zero `TxStateClones` and that journal capacity is reused.
- `staged_values_are_visible_to_later_chain_members_and_skip_after_commits_once`
  checks that later chain members and `skipAfter` see the staged values.
- The bench `transaction_crs` showed identical fingerprints and about 4 700
  fewer allocations per request on every case. In an interleaved A/B,
  clean_get p50 dropped by about 15 %.
- The FTW inventory was unchanged.

Re-evaluate this page if `TxState` gains a field that a unit can mutate, or if
any new TX mutation site appears. Such a field must be journaled or
checkpointed, or rollback stops being exact. Also re-evaluate it if chain
frontier evaluation, unit commit admission, or the retained-byte model of the
staged state changes.

## Resolution references

- Hotspot H2, plan task T12b (`docs/superpowers/plans/2026-11-07-waf-second-pass.md`),
  spec AC-B3. Related guards:
  [chain ordinary work](chain-ordinary-work-accumulates-per-unit.md) and
  [request-scoped control actions](request-scoped-control-actions.md).
