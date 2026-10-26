---
name: outbound-san-fallback-reason-ownership
description: Keep compact ownership for the foreign-error SAN text fallback unless profiling justifies a different representation.
knowledge_type: intentional-design
---

# Outbound SAN fallback reason ownership

User-directed value review at `895a8efd1` declined tls-08's standalone storage
optimization. Keep `outbound_san_verifier.rs::probe_fallback` copying the suffix
after the first SAN mismatch marker into a separate String.

The complete typed pass precedes fallback and returns borrowed reasons for
ordinary typed SAN rejection. Only lost typed identity reaches Display parsing.
Reusing the rendered owner could save one nonempty suffix allocation but still
moves bytes and may retain a much larger rendering capacity. OIDC discards the
reason after classification; LDAP drops it before awaiting the error reply.
This narrow fallback saving has no demonstrated hot-path significance and fixes
no identified trust-policy or correctness defect.

Preserve typed-first traversal, the depth bound, source-only fallback walk,
marker/suffix bytes, diagnostics and sensitive-data handling. This is a choice
of existing ownership, not permission to suppress or alter errors. Reopen with
evidence that supported dependency failures frequently lose typed identity and
that fallback allocation or capacity is material. Compare consumer lifetimes,
not merely the number of String allocations. No runtime change or fresh benchmark
accompanied this decision; previous task evidence remains in Git history.
