# Original CRS structured wire phase

Six unchanged units from OWASP CRS 4.25.1 (revision
`3b89d5a05322f448b4b74b9cadc5fb05ac6915ad`) exercise PHP wrapper detection
(933200), Java class-loading detection (944260), argument-length chain (920370),
PL1/PL2 score aggregation (949060/949061), and explicit anomaly denial (949110).
Original units are Apache 2.0; copyright CRS project and contributors. The
repository copy of the upstream license is at
`edgion-waf/tests/fixtures/crs-structured-v4.25.1/LICENSE`.

Test-only setup and guards have IDs 9900201-9900203. Production pass defaults
remain unchanged: original block actions report findings and increment scores;
original 949110 enforces the final 403. DetectionOnly uses the same bundle and
forwards attacks unchanged. The raw-source guard must have no findings in either
mode. See the focused fixture test for the six original-byte and bundle pins.
