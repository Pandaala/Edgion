# Evaluate a cached-source hint for JSON token growth

Status: Conditional optimization; P2

The JSON token buffer already reuses 1024 inline bytes and retains grown capacity. Measure geometric-growth waste before adding a hint.

- Use only a validated complete cached-source length as a soft growth hint when expansion is already needed.
- Never eagerly reserve the whole body, trust Content-Length, reduce the existing inline default, or shrink reusable capacity.
- A hint must not reject a required allocation that exceeds it; unknown/live sources retain the existing growth path.
- Charge actual capacities and allocation overlap. Preserve terminal errors, EOF, exact-limit admission, and large-then-small reuse.
- Measure allocation count, requested bytes, copies, and simultaneous peak, rather than capacity alone.

Start from `edgion-waf/src/seclang/json_stream.rs` and its source-lifetime/admission tests.
