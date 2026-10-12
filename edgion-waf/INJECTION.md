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
`tests/acceptance/injection-baseline-2026-09-24.csv` (Apple M4, arm64, macOS 15.5,
optimized build, 1,001 samples per case). It is a regression baseline for this
machine, not a cross-platform performance guarantee. The 32 KiB and 64 KiB
dotted-word cases guard the SQL tokenizer's linear prefix scan: their measured
median times are approximately 98 and 194 microseconds, respectively.

## Attribution and distribution

The SQL tokenizer/folding and HTML5/XSS scanners were adapted from the pinned
Rust reference and checked against the pinned Go implementation above. SQL
fingerprint/keyword maps, XSS event and attribute tables, and offline fixtures
use the pinned Go reference. The Rust reference license retains Nick Galbreath's
original libinjection copyright as well as William Saar's; the Go license
retains Coraza's copyright. Keep both complete BSD-3-Clause texts, including
conditions and disclaimers. Edgion's Apache-2.0 license does not replace them.
New upstream inputs require an updated provenance entry and their notices.

Product and combined container images carry this file and `licenses/` under
`/usr/share/doc/edgion/waf/`. Gateway DEB/RPM packages carry them under
`/usr/share/doc/edgion-gateway/waf/`. Binary artifact directories carry `waf/`
beside the executables; retain that directory when archiving or redistributing
those binaries. No separate raw-binary release archive is currently published
by the release workflow.

Release checks compare actual shipped bytes with the canonical source files:

```bash
python3 cicd/checks/check_waf_distribution_notices.py --image IMAGE
python3 cicd/checks/check_waf_distribution_notices.py --directory EXTRACTED_PACKAGE --prefix usr/share/doc/edgion-gateway/waf
python3 cicd/checks/check_waf_distribution_notices.py --archive BINARY_BUNDLE.tar.gz --prefix waf
```

The image release workflow checks built images before pushing. The standalone
workflow extracts each built Gateway DEB/RPM revision and checks it before
upload. The binary build paths check their output directories before upload or
handoff. For a new distribution format, extract its actual output and pass that
root and notice prefix to the same check. Source presence or staging alone is
not a substitute for inspecting the distributed artifact.
