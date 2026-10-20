# Evaluate prepared WAF IP network masks

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `125e8f24c`.

## Evidence and decision

Gateway WAF transaction execution reaches the IpNetworks arm of evaluate_operator
in `edgion-waf/src/seclang/transaction.rs`. It parses the input address once,
then searches the configured network range until a match. ip_matches checks the
address family and reconstructs a u32/u128 mask from the prefix for every tested
same-family network, then masks the input and stored base.

The base is already normalized: parse_network in semantic.rs validates the prefix
and returns mask_ip(address, prefix); compiler.rs copies this address and prefix
into CompiledIpNetwork. Do not describe normalization as missing. The candidate
is to prepare the mask or another equivalent compact comparison representation
once at compilation. Removing the redundant base mask is a separate local
simplification, not evidence that a larger prepared representation is worthwhile.

CompiledIpNetwork currently contains IpAddr and u8 prefix. Compilation charges
retain_compiler_records::<CompiledIpNetwork>(ip_count), reserves the network vector
and retains it in the immutable generation. A prepared representation can change
record size/alignment, compiler retained/peak storage and work. Measure actual
candidate layouts and near-limit admission; no particular size increase or
request-path speedup has been established.

The user decision is whether measured savings for realistic network lists justify
the representation and admission tradeoff, or whether to retain compact prefixes.
Do not introduce a per-request cache, another IP library, a workspace dependency
into this leaf, or an unrelated radix-tree/list-reordering project.

## Acceptance for later implementation

- Preserve IPv4/IPv6 family distinctions, default host prefixes, zero and maximum
  prefixes, host-bit normalization, invalid configuration rejection, first-match
  short circuit and invalid input returning no match.
- Preserve operator negation, transformations, work charges and failure order.
  Do not change Gateway policy, response behavior, logs or metrics.
- Compare actual compiled record size, retained/peak/work costs and exact versus
  one-below compiler budgets. Keep old immutable generations valid during reload.
- Differentially test all prefix lengths with boundary and high-bit addresses,
  mixed-family lists, mapped IPv6 addresses, empty/no-hit and late-hit lists.
  Measure small/common lists and longer near-miss lists separately; input parsing
  remains part of runtime cost and must not be omitted from end-to-end claims.
- Run leaf/Gateway WAF tests and project checks after an independently reviewed
  implementation plan. The original transaction ownership and budget contract is
  in `skills/04-review/waf/regression-guards/fused-json-inspection.md`.

This record delivers no IP runtime change, measurement or acceptance-policy change.
