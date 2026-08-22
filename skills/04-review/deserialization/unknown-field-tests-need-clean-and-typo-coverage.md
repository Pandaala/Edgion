# Unknown-Field Tests Need Clean and Typo Coverage

Unknown-field detection is a schema-dispatch contract, not only an error-path
contract. A focused regression test for a tagged resource family must prove
both directions for every supported variant:

1. A representative clean document produces no findings.
2. The same document with one controlled typo reports the exact expected path.

Checking only the typo case can hide a schema branch that reports the expected
typo together with false positives on valid fields. Checking only one variant
can hide drift in a newly added or renamed discriminator branch.

Public validation-endpoint coverage must additionally assert the
`unknownFieldCheck` discriminator. `unknownFields` alone cannot distinguish a
performed walk with no findings from a resource kind for which the walk was
not available.

For ConfigData and LinkSys, keep the production-schema unit matrices aligned
with every typed variant/provider, and retain an integration assertion for the
Admin validation response shape.
