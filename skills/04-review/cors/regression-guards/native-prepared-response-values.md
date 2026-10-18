---
name: native-prepared-cors-response-values
description: Native CORS prepares fixed wire values after validation and borrows them during header preparation.
knowledge_type: regression-guards
---

# Native CORS prepared response values

GapiCorsFilter consumes HTTPCORSFilter only after its canonical validation passes.
It retains origin patterns and prepares method/header policies, exposed-header
text and Max-Age once. It does not retain a duplicate raw configuration. Fixed
lists preserve order and the comma-space delimiter; missing or empty lists omit
the header. Credentialed lists containing an exact wildcard reflect the
corresponding request header, preserving missing versus empty values.

Header pairs borrow fixed values and the caller-owned origin. Reflections own
their values before session mutation; the returned lifetime must not borrow the
session. Cookie/Authorization presence remains a live decision for wildcard
origins without configured credentials. Configured credentials always echo the
origin. Origin matching, validation, Vary append, header ordering and errors are
unchanged. Native methods/allow-headers remain emitted on actual responses as
well as preflight; Max-Age remains preflight-only. This optimization does not
establish or change that native response policy.

The native filter unit tests cover fixed/empty/missing/wildcard policies,
credential presence, reflection values and both response phases. Warmed
header-pair construction allocates zero for fixed values and two for two
nonempty reflected values. This excludes origin acquisition and final header
insertion, and makes no end-to-end throughput claim.

Re-evaluate when adding configuration fields, changing canonical validation,
reflection ownership, origin credential policy or header-pair capacity.
