# Evaluate LDAP bind DN template preparation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `9df8ca5cc`.

## Evidence and decision

LdapAuth::build_bind_dn in `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`
rejects an empty username, calls ldap3::dn_escape once, then uses String::replace
for the literal {username} placeholder in BindDnPlan::Template. The constructor
already owns the immutable template. Template validation requires at least one
placeholder but imposes no occurrence-count cap.

Locked ldap3 0.12.1 returns borrowed input from dn_escape when no escaping is
needed; avoid attributing an unconditional escape allocation to this path. The
final bind DN is an owned String used by the separate LDAP bind exchange.

Choose between keeping the current standard-library replacement and preparing
placeholder byte offsets/ranges in the existing generation-owned BindDnPlan.
Prepared positions can avoid searching each request but retain metadata per
instance, including unused configurations. A range design must keep the owning
template and avoid self-referential slices; copied literal fragments would add
a different memory cost. The final DN String and any necessary escaped username
storage remain. No candidate implementation or benchmark has run.

Compare short common templates as well as long and repeated-placeholder templates.
Network connection/bind work may dominate actual requests; do not infer end-to-end
benefit solely from fewer scans. This proposal does not authorize connection
pooling, credential-result caching, a new template language or a generic template
engine. Preserve the existing LDAP literal-placeholder contract.

## Semantic boundaries

Replace every non-overlapping literal occurrence, including adjacent occurrences,
with the same once-escaped username. Do not recursively interpret replacement
text: a username containing {username} remains literal user data. Preserve all
other template bytes, matching case, RFC 4514 escaping, UTF-8 boundaries, leading
hash/space and trailing-space escaping. Empty-username rejection is unchanged.
The AttributeBase branch keeps its current attribute=value,base_dn composition.

Preparation must stay within the immutable plugin generation, with old instances
retaining their own template policy. Do not introduce a global plan cache, a new
placeholder-count limit or altered configuration rejection as an incidental
optimization. If a measured design needs a new resource bound, seek an explicit
scope decision rather than silently narrowing accepted templates.

Keep credential parsing/header priority, exact password bytes, dial-time egress,
TLS policy, fresh per-request bind semantics, timeout and cleanup behavior,
fixed rejection messages and sensitive logging unchanged. No DN/password/user
values may be added to diagnostics or benchmark output.

## Acceptance for a later implementation

Use IMPL with an exact allow-list. Compare full output against the existing
replace path for single/multiple/adjacent placeholders, UTF-8 literal text,
metacharacter usernames, leading/trailing spaces, literal {username} in a username
and empty-username rejection. Distinguish accepted configurations from defensive
constructor/helper bypass cases. Preserve existing injection-escaping tests and
verify old/new plugin generation isolation and the AttributeBase branch.

Measure request CPU, transient allocations and requested bytes together with
construction work and retained template/metadata capacity across many instances,
including unused plans and high occurrence counts. Keep the canonical LDAP escape
function and final SDK ownership; fewer scans are not proof of lower total cost.

Fresh unchanged-code verification passed five build_bind_dn tests covering
escaping, spaces, UTF-8, empty rejection and template substitution. They do not
cover every proposed case or prove candidate equivalence. No benchmark ran.
Original authority: `skills/04-review/ldap-auth/intentional-design/per-request-connection-accepted.md`,
`skills/04-review/ldap-auth/regression-guards/dial-time-egress-policy.md` and
`skills/04-review/security/review-evidence/credential-copy-ownership.md`.
