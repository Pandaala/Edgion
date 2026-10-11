---
id: ARCH-042
status: candidate
kind: architecture-debt
priority: P3
partition: PART-383
analyst: /root/analyze_conversion
independent_verification: confirmed-with-revision
---

# Materialize only the requested SAN projection

The certificate extractor owns display SAN strings for ClientCertInfo and typed
PresentedSan identities for the canonical allowlist validator. Distinct types
are necessary: display prefixes must never determine certificate identity type.

In tls/runtime/backend/cert_extractor.rs:74-134, extract_sans takes tuple .0
and extract_presented_sans takes .1 from extract_san_sets. That shared parser
always constructs both owned vectors. DNS entries receive two owned string
copies; IP, email and URI entries receive display formatting plus typed values.
extract_other_names:137-162 likewise constructs both formatted and typed OID
values. The projection is selected only after allocation.

Actual production consumers use one projection per invocation:
extract_client_cert_info_from_cert:19-61 feeds downstream completion
(tls_pingora.rs:363-371) and HeaderCertAuth (plugin.rs:250-259); the mTLS
callback:228-244 requests typed SANs; extract_sans_from_ssl:290-295 supplies
backend certificate display SANs to peer_builder.rs:235-238. No inspected
production caller consumes a pair. The allowlist callback first extracts client
info and then typed SANs, discarding one populated projection on each call.

These connection/handshake and HeaderCertAuth request paths perform avoidable
allocations proportional to the certificate SAN entries. This is a demonstrated
source ownership cost; no aggregate heap, CPU, latency or throughput measurement
was executed. No trust-policy defect is claimed.

Keep one private SAN enumeration and shared OtherName decoder, selecting display
or typed output before constructing owned values. A small private projection
parameter or statically dispatched sink is sufficient; no public SAN framework,
certificate cache, session change or runtime owner is needed. This does not
eliminate repeated DER parsing or change callers' independent invocations.
Retaining the pair and accepting the cost is the bounded alternative. Deriving
display from typed identities still needs to avoid unnecessary typed ownership;
two independent parsers risk interpretation drift. Parsing display prefixes back
into typed identities violates the security boundary.

Preserve exact display formatting, order, duplicates, invalid-IP skipping and
empty results; distinguish DNS/IP/email/URI/OtherName. Preserve OIDs and current
BER acceptance/rejection: complete-input consumption, explicit wrappers,
supported textual/scalar values, preservation of embedded NULs without
truncation, and malformed/unsupported encoding rejection. Subject/CN/fingerprint
extraction stays unchanged. Typed values feed the canonical allowlist validator;
display remains presentation.
Preserve chain preverification, leaf-only whitelist, verification modes,
redaction, one-owner audit emission, panic containment, handshake ownership and
backend enforcement. No new request logs or metrics.

The original typed-san-display-prefix-not-dns, mtls-leaf-only-whitelist-verification
and ssl-ctx-exdata-handoff-audit-ownership decisions were read and verified
against current callers. They require distinct semantics, not both allocations.
ARCH017 prepared backend material, ARCH034 leaf-DER ownership and ARCH040/041
HTTP transport/startup policy have different causes. Existing tasks/decisions
were searched; no paired-materialization requirement was found.

Future verification should compare both projections with real mixed-SAN
certificates, duplicates, IP forms, OtherName OIDs and supported/unsupported BER.
Retain extractor/callback/TLS wiring contracts and show that choosing one output
never constructs the unrequested output vector or its projection-only owned
values. Existing tests were inspected, not run. Baseline primary hash:
0126fb49bc9056c718474b44ad0d70de314e19b7690f284c78347c831a382dbf.
Supporting full/selected read scopes are recorded in REPORT-PART-383.json.
Independent verification is pending; production changes are not authorized.
