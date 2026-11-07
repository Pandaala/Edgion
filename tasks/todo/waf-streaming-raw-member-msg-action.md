# Decide whether streaming raw-body WAF members may carry `msg`

Priority: P3   Area: plugin:waf (edgion-waf compiler)

## Problem
`prefix_actions_supported` and `is_streaming_raw_member` in
`edgion-waf/src/seclang/compiler.rs` share one action allow-list
(`is_streaming_safe_action`) except for `Msg`: the request-prefix path accepts
`msg` on a member, the streaming raw-body path does not. The asymmetry predates
the 2026-11-07 optimization pass, which deliberately preserved it instead of
unifying the two lists. `Msg`/`LogData` templates are expanded at finding time
(`transaction.rs` `expand_template`), which is the likely reason the streaming
path excludes `msg`, but no recorded decision confirms that.

## Proposed change
Decide and record one of:
- the exclusion is intentional: document the reason on `is_streaming_safe_action`
  and in `skills/04-review/waf/`, keep the code as is; or
- streaming raw members may carry `msg`: drop the `|| kind == ActionKind::Msg`
  special case so both paths use the same list, and add a streaming test whose
  rule has a templated `msg`.

## Must preserve
- Prefix-path eligibility is unchanged unless the decision says otherwise.
- The regression guards under `skills/04-review/waf/regression-guards/` for
  request-prefix and streaming inspection.

## Verification
- `cargo test -p edgion-waf --lib compiler::tests` and `transaction::tests`.
- The `WafDynamic` Gateway integration scenario if the streaming list changes.
