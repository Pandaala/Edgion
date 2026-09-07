---
name: rwlock-unwrap-review-rule
description: Decision rule for RwLock/Mutex .unwrap() in reviews — only a possible panic inside a write critical section constitutes real risk; otherwise it is a false positive or over-graded.
knowledge_type: review-evidence
---

# RwLock `.unwrap()` Review Decision Rule

## Core Conclusion

**Do not flag every `.unwrap()`; only check whether the write critical section may panic.**

The only trigger for `std::sync::RwLock` poisoning: **a thread holding the write lock panics while holding it**.
A panic in a thread holding the read lock does not poison the lock.

## Decision Steps

1. Find all `write().unwrap()` call sites
2. Look at what is executed **while the write lock is held**
3. If the critical section only does `HashMap::insert`, `HashMap::clear`, field assignment, etc., these operations **cannot panic** → no real risk; false positive or over-graded
4. If the critical section contains user-data-driven parsing, `unwrap()`, `assert!` etc. → there is real risk; worth fixing

## On Read Locks

`read().unwrap()` only fails after the lock has been poisoned by a write path. If the write critical section cannot panic (see above), these `unwrap`s likewise carry no real risk.

## Reference Case

The 12 `unwrap`s in `processor_registry.rs` were rated P0; in reality the write critical sections only do:
- `map.insert(kind, processor)` — cannot panic
- `map.clear()` — cannot panic

Conclusion: there is no real Poison risk currently; can be downgraded to P2 (defensive improvement, no dedicated fix needed).

---

## `std::sync` in async Context — Common False Positive

**Report pattern**: "`std::sync::Mutex/RwLock` is used in an async function; should switch to `tokio::sync`".

**Decision steps**:

1. Check whether the guard crosses `.await` — if the critical section ends before `await` (temp variable, block scope), **no problem**.
2. Check poisoning risk (see above section) — if the write critical section cannot panic, poisoning will not trigger and switching gives no substantial benefit.
3. Only consider `tokio::sync::Mutex` if you really need to hold the lock across await; but this complicates synchronous trait methods (need `block_on`), often degrading the interface.

**Why not introduce `parking_lot`**: its advantage is "no poisoning", but if the write critical section cannot panic at all, poisoning is dead-code path; introducing `parking_lot` only swaps `.unwrap()` for an `unwrap()`-free style with no substantial improvement.

**Edgion controller-layer conclusion**: after audit, the conf_mgr module has no guard-across-await violation; write critical sections are all safe operations; `std::sync` can stay as-is.
