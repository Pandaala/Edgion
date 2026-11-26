# waf5-02: Evaluate companion SecLang request-body anomaly variables

## Implemented baseline

Full inspection exposes the read-only `REQBODY_ERROR` scalar from phase 2. A
completed JSON, XML or multipart syntax fallback yields 1. Successful publication,
including tolerant URL-encoded form decoding, yields 0. The enforcing default
after a detected parse failure is 400. An intervening rule can select a different
status. DetectionOnly can score the complete fallback and forward the request.
The gateway keeps its independent limit and transport failure handling.

See the [parser outcome specification](../../skills/04-review/waf/mandatory-design/request-body-parser-outcome.md) and the
[gateway error attribution contract](../../skills/04-review/waf/mandatory-design/engine-trust-boundaries-and-gateway-contract.md).

## Remaining investigation

The [ModSecurity v3 recommended configuration](https://github.com/owasp-modsecurity/ModSecurity/blob/v3/master/modsecurity.conf-recommended)
provides two concrete enforcement consumers. Rule 200003 tests
`MULTIPART_STRICT_ERROR`; its per-anomaly macros supply diagnostic context.
Rule 200004 tests `MULTIPART_UNMATCHED_BOUNDARY` for the specific value 1,
so a generic Boolean parse-error alias would lose information. Rule 200002 uses
`REQBODY_ERROR_MSG` in log data while its decision uses the implemented scalar.
The bundled CRS 4.25.1 profile has no direct references to those three variables
or `URLENCODED_ERROR`.

Defer adding companion targets until a supported policy needs them. Edgion already
rejects ambiguous multipart field ownership using the existing parse-error path.
The bundled profile has no consumer for a separate strict-validation signal.

For an import of [Coraza rule 200003](https://github.com/corazawaf/coraza/blob/856f68a904cd9ee960060ab457c491531957126d/coraza.conf-recommended#L103-L122),
specify the supported producer version and its complete strict-validation conditions.
The original rule also expands duplicate-header and invalid-quoting diagnostic macros.
Edgion validates macro sources separately from rule targets. A scalar alone would
not allow that rule to compile. Verify the unmodified policy and its observable
decisions before claiming import compatibility. ModSecurity's boundary rule 200004
requires a separate multi-valued contract.

Identify a concrete rule and operational need before adding a companion variable.
Candidates include `URLENCODED_ERROR` for tolerated encoding anomalies, the
`MULTIPART_*` flags for specific framing anomalies, and processor-error aliases
or bounded diagnostic messages. Verify each candidate against its upstream
producer and consuming rule. Specify timing, limits and redaction before enabling it.

Preserve the literal form value for tolerated invalid escapes and the existing
`REQBODY_ERROR=0` outcome. Any distinct URL-encoding anomaly signal needs its own
tests. Partial structured-field recovery is tracked separately under waf5-01.

Origin: F-S3-2, 2026-11-07 body-parser review.
