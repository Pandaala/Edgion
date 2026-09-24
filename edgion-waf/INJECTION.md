# Native libinjection operators

`@detectSQLi` and `@detectXSS` run in the native SecLang transaction pipeline.
They use bytes after the rule's selected target and transformations. SQLi uses
the libinjection fingerprint and false-positive whitelist; a rule with
`capture` writes the fingerprint to `TX.0`. XSS checks all five HTML tokenizer
contexts and does not create a capture. Chains and negation use the existing
transaction commit rules.

The behavioral reference is `github.com/Coraza-Web/libinjection-go` tag
`v0.3.2`, commit `f827564b9f5bf446a038deabd535361e114109ce` (BSD-3-Clause).
The initial Rust translation was informed by `github.com/Karneades/libinjectionrs`
commit `597f581013cc52656069ca748c7f666f28fba7ac` (BSD-3-Clause); its
tokenizer, folding, and HTML5 code was adapted and checked against the pinned
Go behavior. Both license texts are in `licenses/`. Neither project is linked
as a production dependency. The 9,352 SQL map entries, 432 XSS event names,
other XSS tables, and offline parser fixtures can be checked against a local
Go checkout:

```bash
python3 edgion-waf/tools/verify_libinjection_go_v0_3_2.py /path/to/libinjection-go
cargo test -p edgion-waf --lib
```

The default tests include 61 SQLi/XSS verdict fixtures and 431 ASCII
token/folding/HTML5 fixtures. Four upstream parser fixtures contain non-ASCII
characters and are checked locally with the upstream checkout rather than
stored in the repository:

```bash
LIBINJECTION_GO_FIXTURES=/path/to/libinjection-go/tests \
  cargo test -p edgion-waf --lib compare_all_upstream_token_fixtures -- --ignored
LIBINJECTION_GO_FIXTURES=/path/to/libinjection-go/tests \
  cargo test -p edgion-waf --lib compare_upstream_fixtures -- --ignored
```

The operator reserves weighted scan work before each value. SQLi can parse a
value in up to five dialect/quote contexts; XSS checks five HTML contexts. The
detector charge is linear in value length and accumulates across rule members
and phases in one transaction under `max_operator_work_bytes`. The existing
per-member operator work checks, chain-branch checks, and request-body stream
checks retain their separate local accounting against the same configured
limit; they are not included in the detector cumulative counter. Detector
input is capped at 4 MiB, matching
the default runtime value limit. A 4 MiB value fits under the default 64 MiB
operator work limit for one detector evaluation. This is a resource guard, not
a claim that a weighted byte equals a CPU instruction. No request bytes are
retained by the scanners, and no detector-specific request logging is emitted.

The end-to-end transaction benchmark includes request construction, target
selection, transformations, operator evaluation, and finding publication:

```bash
cargo bench -p edgion-waf --bench injection_regression
```

The first measurement is recorded in
`acceptance/injection-baseline-2026-09-24.csv` (Apple M4, arm64, macOS 15.5,
optimized build, 1,001 samples per case). It is a regression baseline for this
machine, not a cross-platform performance guarantee. The 32 KiB and 64 KiB
dotted-word cases guard the SQL tokenizer's linear prefix scan: their measured
median times are approximately 98 and 194 microseconds, respectively.
