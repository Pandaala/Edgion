# Evaluate a Conservative Regex Literal Prefilter

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P3 |
| Activation | Regex execution is a measured request-path bottleneck |

## Goal

Evaluate compiler-extracted literal requirements as candidate-only prefilters
for the accepted `regex-automata` subset without introducing false negatives or
reordering WAF execution.

## Work

- Extract a literal only when the compiler can prove
  `regex_match(input) => literal_candidate(input)`.
- Keep patterns without a proven necessary literal in an always-evaluate
  bucket.
- Reject unsafe extraction around alternation, optional branches, look-around,
  Unicode/case behavior, captures, and transformations.
- Execute exact regexes in native source order after candidate collection.
- Measure compile memory, candidate density, regex calls avoided, and
  end-to-end request latency.

## Exit gate

Enable the prefilter only after proof-oriented and randomized differentials
show zero false negatives and representative workloads show a material
end-to-end gain.
