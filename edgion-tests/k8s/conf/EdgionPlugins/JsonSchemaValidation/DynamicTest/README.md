# JSON Schema Validation Hot-Reload Test

This scenario verifies that invalid JSON Schema configurations fail closed and
that every accepted update publishes a newly compiled immutable validator.

The state machine is:

1. invalid external reference: `Accepted=False`, traffic fails closed with 500;
2. valid v1 schema: `Accepted=True`, only `{"version":"v1"}` is accepted;
3. invalid external reference again: `Accepted=False`, traffic fails closed;
4. valid v2 schema: `Accepted=True`, only `{"version":"v2"}` is accepted.

The two invalid phases ensure both invalid-to-valid recovery and
valid-to-invalid-to-valid recovery are covered without restarting the Gateway.
