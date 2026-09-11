# Publish Bounded JSON Values and Paths

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `02-incremental-json-tokenizer.md` |

## Goal

Convert tokenizer events into the exact bounded JSON variable views consumed
by the WAF transaction without building a DOM or retaining previous values.

## Work

- Maintain a bounded object/array path stack.
- Publish scalar values and collection-path metadata in deterministic source
  order.
- Define duplicate-key and array-index behavior according to the frozen
  contract.
- Reuse transaction-owned matcher scratch for eligible phrase groups.
- Release token/value storage immediately after all subscribed rule groups
  consume the value.
- Preserve only bounded capture and transaction metadata needed by later
  rules.
- Add a completion summary containing counts and limit outcomes only.

## Tests

- Differentially compare published paths, values, order, matches, findings,
  and interventions with the existing complete JSON body processor.
- Cover duplicate names, empty names, nested arrays/objects, escaped names,
  scalar roots, and every chunk split.
- Assert that payload bytes never enter `Debug`, error text, or metrics.

## Exit gate

All supported JSON rule units produce the same transaction-visible values and
verdicts as the bounded complete-body reference while retaining only bounded
current-token and path state.
