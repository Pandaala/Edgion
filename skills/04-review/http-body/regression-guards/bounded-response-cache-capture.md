# Bounded public-origin response cache

ResponseCache is explicitly nominated by a cold-compiled request plugin and
bound once after ordinary request plugins/body policy succeed. Authentication
and quota denial precede lookup. Runtime certificates include response companions
and preserve unsafe classification across conditions, reuse and policy masking.
The initial supported KeyAuth profile removes header credentials and publishes no
consumer fields. Authorization and Cookie always bypass.
Range, If-Range, If-Match, If-None-Match, If-Modified-Since and
If-Unmodified-Since also bypass before lookup or capture through one canonical
header-presence admission check; ordinary origin handling owns their semantics.
Real-tail repeated requests against a populated cache must show zero additional
LRU lookups, no pending capture lease, and successful ordinary origin dispatch.

The cache owns one private preadmitted arena, not response BodyStorage. Defaults
are 1 MiB per arena, 16 KiB/64 fields metadata, 16 MiB concurrent pending and
128 MiB total process ownership. Body callbacks copy bounded chunks without
allocation or tracing. Overflow, errors, cancellation and trailers discard the
private fill while the origin response keeps its ordinary forwarding behavior.
Seal the actual final onion header at the canonical response_head_will_commit
boundary, before transport preparation. The seal requires the exact captured
Cache-Control, Date and Age contract. Terminal publication retains all integrity,
policy, origin, epoch and body checks; only a valid singleton Date may differ
after the locked H1/H2 transport rewrites it. A missing seal or recapture cannot
reuse an earlier seal. Plugin-side Date changes remain strict failures. Freshness
and replay Age stay bound to the origin timestamp, never the transport timestamp.
Publish only after actual typed clean terminal state and bounded finish_body.
H1 proves local flush; H2 proves accepted EOS, not peer receipt.

The index is canonical ShardedLru. Admission precedes owned metadata/header/body
allocations. Decoded hit headers remain charged until context teardown because
Session may retain response_written. Bytes::from_owner keeps arena/control leases
alive through retained slices after eviction/replacement. Strict metadata decode
consumes every record; the seal/replay/publication contract checks gather first
case-insensitive field values into fixed borrowed slots in one complete metadata
pass. Finding all contract fields never permits skipping a malformed suffix.
Decoded origin headers use the same private supported-header/Date/Age parser as
capture. Decode checks integrity without reading the wall clock or constructing
a discarded Freshness value; stored monotonic origin freshness remains the sole
expiry/replay-age source. This preserves validation because the former decode
call used equal request/header instants: corrected-age addition had zero delay,
and wall-derived apparent age and TTL clamping could not reject the metadata.
Capture still samples its original clocks and applies the same age arithmetic.
Vary parsing remains independent at each boundary; this does not implement the
deferred boundary-local Vary reuse proposal.

Absent and empty values remain distinct; final transport still permits only its
separately validated singleton Date replacement. Corruption evicts only the matching current identity and
falls through before final headers. Postheader failures never fall back to origin.
Current response headers, trace/CORS/dye onion and veto execute once on replay.

Keys frame final method/path/query, validated Host/authority, effective normalized
headers, raw POST bytes SHA/length, selected successful GraphQL query proof,
runtime identities, publication epoch, Service/discovery and policy proofs.
Connection nominations reuse the canonical token iterator after bounded grammar
validation. A fixed borrowed index retains case-insensitive first-occurrence
membership within each synchronous lookup. Configured variations share one index
within their nonempty synchronous loop, freshly parsed after any request-body
scan; it never crosses an await, header mutation or callback boundary. Managed trace and integration-only
access_log are excluded; arbitrary x-* headers remain material. Vary can refer
only to represented fields. Header-name certificate hashes retain the same domain
prefix and ASCII-lowercased byte stream; bounded stack chunks batch digest updates
without changing normalization, admission limits or certificate identity.
Contextual upstream-authority templates bypass before lookup; eligible literal
templates reuse canonical bounded rendering and actual consumed-result stamps.
Actual discovery/TLS/traffic/authority reports must match the expected contract
before fill and commit, without new selection or snapshot reload for reporting.

Publisher retirement atomically clears visibility and fences pending writers.
An admitted current epoch guard is standing infrastructure even when the index
is empty. Arc clones do not add charges. Successful replacement can leave a
distinct retired guard pinned by requests; refusal can retain the same inactive
guard in holder and old token with no extra charge until later replacement.
Residue therefore checks allocated minus infrastructure, pending, entries and
reader pins, rather than demanding total zero or assuming every alias is residue.
Numeric statistics expose neither keys/digests nor payload/header values.

Acceptance requires fresh actual request-denial and response-veto effects,
H1/H2 terminal/flush/cancellation, allocation/lease/corruption, generation and
same-read origin tests plus static/dynamic real-origin integration. Historical
C0 acceptance does not establish a completed public feature. Reevaluate on any
owner, callback order, metadata format, certificate, key, transport or ledger
change; fixture headers/body collection alone are not terminal trailer proof.
