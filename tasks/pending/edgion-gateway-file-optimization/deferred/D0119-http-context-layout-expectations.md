# D0119 — HTTP Context Layout Expectations

- Status: resolved externally in `2c01a075`
- Severity: low (stale regression expectations)
- Discovered in: B0207 broader context verification
- Resolution: the exact context layout expectations now match 72/248/1120 bytes.

Independent diagnosis reproduced three fixed-size failures caused by committed
HTTP context changes predating B0207. External commit `2c01a075` corrected the
three exact expectations without suppressing the 1280-byte budget or allocation
checks. Fresh `cargo test --locked -p edgion-gateway --lib 'ctx::'` verification
passed 94 tests with one ignored. The complete campaign audit of `ctx.rs`,
including `TlsConnMeta.sni` masking, remains pending in the file ledger.
