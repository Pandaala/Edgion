---
name: dsl-step-budget-not-cpu-bound
description: Use when reviewing findings that claim the DSL VM's `max_steps` budget bounds CPU time per execution, or that regex opcodes / `CallBuiltin` "should each consume more than one step" because they do O(input_len) work. The behavior is intentional and documented; the per-opcode CPU is bounded by `max_string_len` and per-builtin output caps, not by `max_steps`.
knowledge_type: intentional-design
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index, and
> [`../../../01-architecture/02-gateway/dsl-engine-security-model.md`](../../../01-architecture/02-gateway/dsl-engine-security-model.md)
> for the canonical breakdown of every DSL VM cap.

# DSL VM step budget counts opcode dispatches, not CPU time (intentional)

**Recurring finding shape**: a reviewer reads
`edgion-gateway/src/plugins/http/dsl/lang/vm.rs` and notices that:

- `state.tick_step()?` deducts exactly **1** step before each opcode dispatch.
- `OpCode::Matches` / `RegexFind` / `RegexReplace` (`vm.rs:578-645`) invoke
  `regex::Regex::is_match` / `find` / `replace_all` on strings up to
  `max_string_len` bytes — O(input_len) work counted as 1 step.
- `OpCode::CallBuiltin` (`vm.rs:765`) deducts `tick_call()` from the separate
  call budget, but the builtin body (`json_get` recursing 32 levels, `split`
  building a Vec, `url_encode` / `base64` iterating chars) is also O(input_len)
  per single step / call.

The reviewer then flags this as either (a) a missing check ("regex opcodes
must charge proportional to input length"), or (b) an undocumented security
model gap ("the comment says 'every instruction costs 1 step' but CPU is not
actually bounded by `max_steps`").

**Why this is intentional, not a defect**:

1. **The four-cap product *is* the CPU bound**. The realistic worst-case CPU
   for a single DSL execution is approximately
   `max_steps * max_string_len` for regex-heavy scripts and
   `max_call_count * max_string_len` for builtin-heavy scripts, additionally
   capped by:
   - each precompiled regex's `dfa_size_limit(1 << 20)` (`vm.rs` regex
     constant rebuild path — see `dsl-engine-security-model.md` table row
     "VM init (regex precompile)").
   - per-builtin output caps via the shared `checked_value_str` /
     `checked_value_opt_str` helpers (`builtins.rs:46-56`) plus per-call hard
     ceilings (`split` pre-allocation estimate, `range` 1000-element ceiling,
     `http.call` response header truncation at `max_map_size`, etc.).
2. **Charging proportional steps would be theatre, not protection**. The
   regex crate already enforces its own DFA size limit; a "step cost
   proportional to input_len" would not lower the practical CPU bound (it is
   already `max_string_len`-bounded inside `regex`), but would make `max_steps`
   harder to reason about (one regex on a long string can blow the whole step
   budget, killing scripts that the operator intentionally sized for many
   regex passes).
3. **No tenant can exceed the four-cap product**. Even if a script chains the
   maximum number of regex / builtin operations on max-length strings, the
   worst-case CPU is still finite and operator-tunable via the four numeric
   caps; there is no path that bypasses all four.

**Code-side documentation**: the module-level comment block at the top of
`edgion-gateway/src/plugins/http/dsl/lang/vm.rs` explicitly calls this out
("Note on the step budget …"), with a reverse-link to
[`dsl-engine-security-model.md`](../../../01-architecture/02-gateway/dsl-engine-security-model.md).

**Decision criteria during review**:

- Finding asks to add a per-input-length step charge inside `OpCode::Matches`
  / `RegexFind` / `RegexReplace` / `CallBuiltin` → **reject**, the four-cap
  product is the CPU bound and adding a proportional charge changes
  operator-visible script budgeting without lowering the worst-case CPU.
- Finding says the security-model comment is misleading → **already
  resolved** (2026-05-26 comment update in `vm.rs:1-22`). If you see the
  comment block without the "Note on the step budget …" paragraph, that
  finding is a true regression of this fix; otherwise it is a reverse
  regression of a closed finding.
- Finding proposes changing `tick_step` to take a weighted cost parameter →
  **reject**, this is the same shape as the previous bullet and would
  bake the regex-step weighting into the VM's public budget contract. The
  weighting (if ever needed) belongs in tighter `max_string_len` /
  `max_call_count` defaults, not in opcode-by-opcode step costs.

**Verdict**: the step budget's role is to bound opcode dispatches.
Per-opcode CPU is bounded separately by `max_string_len`, per-regex DFA size
limits, and per-builtin output caps. The combination — not `max_steps`
alone — is the runtime CPU bound. Findings that read `max_steps` as a CPU
bound are reading the wrong cap; point them at
`dsl-engine-security-model.md` and the `vm.rs` module-level note.
