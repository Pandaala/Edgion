# Provider cooldown survives a concurrent success

Passive ejection distinguishes a consecutive-failure window from an explicit
provider cooldown such as AI credential `Retry-After`. Both deadlines live in
the same mutex-protected state, and admission uses the later deadline.

A normal success from a request that was already in flight resets failure
counters and clears only the failure-ejection window. It must not cancel an
explicit cooldown installed by another request. When that explicit state
exists, normal success also leaves its generation and exclusive probe lease
unchanged. After the effective deadline, the existing single half-open probe
remains required; only its generation-valid success closes both windows.

Do not restore a single undifferentiated `open_until` or make ordinary success
clear the explicit deadline. Tests must cover both directions: concurrent
success preserves provider cooldown and an already-acquired probe, while
success still clears an ordinary failure ejection.
