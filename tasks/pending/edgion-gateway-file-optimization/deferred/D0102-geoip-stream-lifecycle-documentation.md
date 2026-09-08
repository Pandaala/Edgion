# D0102 — GeoIP Stream Lifecycle Documentation

- Status: pending
- Severity: medium (operator documentation correctness)
- Discovered in: B0152 stream GeoIP plugin analysis
- Canonical task: [Align bilingual GeoIP stream lifecycle documentation](../../../todo/align-bilingual-geoip-stream-lifecycle-documentation.md)

The English and Chinese GeoIP operations guides still describe all L4 execution
as Stage 1 and all denial as lacking protocol logs. Current listener, TCP/UDP
route and TLS compatibility call sites have distinct lifecycle and logging
ownership. Correct both languages together in a separate documentation batch;
the metrics inventory itself is already accurate.
