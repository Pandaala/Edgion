---
name: shared-tls-fingerprint-encoding
description: Use when reviewing LinkSys provider TLS policy and Secret fingerprint encoding.
knowledge_type: regression-guards
---

# LinkSys providers share TLS fingerprint encoding

## Conclusion and scope

Five persistent providers use one canonical TLS policy encoder. Those
providers, Webhook, and prepared HTTPDNS clients use one TLS material encoder
in Common's `link_sys_core/fingerprint.rs`. HTTPDNS, Webhook, Redis, Elasticsearch and Kafka use its
single inherited-axis encoder, applying local verify/validation/client overrides
before hashing only consumed global policy, material and proof outcomes. Native
inactive HTTP TLS consumes no inherited axes. Gateway re-exports those helpers.
Redis implicit rediss consumes inherited axes through an empty local policy;
its existing absent/disabled local config identity distinctions remain intact.
Controller HTTPDNS preparation consumes the same implementation through Common.
Provider-specific configuration and credential hashing remain local. Webhook
filters disabled TLS before hashing its prepared connection. HTTPDNS hashes its
redacted configuration plus enabled TLS material; disabled TLS policy fields can
still change its configuration identity, while disabled Secret bytes do not.

## Rationale and evidence

The provider-local encoders previously repeated the same option tags, ordered
SAN/reference/CA lists, and sorted Secret data-key encoding. A shared helper
now lives in `edgion-common/src/link_sys_core/fingerprint.rs`, allowing both
Controller and Gateway to reuse it without another encoder. Material name,
namespace and the exact reserved `edgion.io/controller-ca-source` annotation
are included because owning reference proof consumes them. Other metadata,
including UID and resourceVersion, remains excluded. Byte-identical PEM with a
different object identity or source kind must invalidate material reuse. This
refines the historical metadata-exclusion rule; unchanged UID/RV-only material
does not change consumed-material fingerprints. The global publication setter has
a separate full-metadata equality contract documented below. HTTPDNS material
identity is separate from its redacted
address-cache fingerprint; see [prepared HTTPDNS runtime](httpdns-prepared-runtime.md).

## Verification and re-evaluation

Shared helper tests cover policy variants, enabled/disabled material, changed
Secret bytes, consumed identity/source markers, and UID/RV exclusion. Incoming
owning proof must precede equality/RV reuse shortcuts; a hash alone does not
prevent unchanged-RV replay with altered invalid references. HTTPDNS now checks
actual incoming owner/local/global authority in its candidate before both outer
reuse sites. WebhookCandidate now checks the same incoming authority before
digest and same-RV reuse and uses its captured snapshot for construction.
RedisCandidate prepares actual source/local/consumed inherited authority before
the persistent runner can enter reuse or zero-live planning. The same immutable
candidate persists across initialization retries. Its common PreparedRedisTls
adapter feeds the shared rustls conversion core once per construction, and redis-rs
retains that connector across reconnects.
Redis and Elasticsearch share Common's PreparedLinkSysTls for immutable actual
ownership, source/whole active pairing and captured-default loans. EsCandidate
checks active inherited authority before replay and supplies one fingerprint to
construction; native absent/disabled ES TLS consumes no inherited axes, even for
HTTPS URLs. Its fixed-header canonical factory owns builder creation and rejects
raw TLS substitutions; bulk/recovery retain the same constructed Client.
KafkaCandidate uses the same immutable actual owner container and validates source,
whole active local pairing and consumed inherited authority before persistent
planning. Its private fingerprint includes the same masked inherited encoder.
Absent/disabled Kafka TLS consumes no globals; enabled System TLS still produces
an explicit verifying connector because rskafka treats None as plaintext. Cold
construction captures one connector Arc; init, probe recovery and SDK broker
reconnects retain that connector without reading new global policy. Other
provider replay gates remain in progress under
`tasks/pending/resource-reference-contract/`. The intermediate Resources
`BoundOutboundTls` loan pairs actual plugin/provider policy, ordered references
and same-field material without copies; Common `validate_outbound_tls_field`
checks actual source identity, cardinality, target name/namespace and exact ConfigMap marker versus
ordinary Secret marker absence. Its local proof does not prove inherited global
axes or establish transport/reuse gating until those consumers migrate.
The owning BoundLocalTlsSelection retains active, inactive or absent actual-field
selection. Its Common validation checks source identity even with no declared
slots; disabled material stays inert. ValidatedLocalTlsSelection has a private
constructor and the pure effective resolver borrows only that proof and one
captured global snapshot. No raw policy or material can replace its local inputs.
This selection does not enable TLS or authorize a transport: owning adapters must
preserve scheme/enable/stage gates, and other transport/replay consumers still need
their separate migration. HTTPDNS absent/disabled policy keeps native system
trust without global inheritance. These are foundation checks, not proof that
every client already uses them.
Global publication now retains original immutable EGC Arcs in Common's opaque
GlobalOutboundTlsSnapshot. The inherited verify, selected validation/CA and client
axes have independent owning proofs; malformed declarations still count toward
conflicts. Borrowed material pairing uses the same private local/global checker.
HTTPDNS and Webhook preparation capture one snapshot for proof, fingerprint and construction;
its encoder borrows global components without fabricating a local TlsConfig or
copying Secrets. Effective proof failures participate only on actually inherited
axes; masked global changes do not alter the private identity. The shared pure
active-local inheritance check does not resolve PEM or emit TLS audit.
Ready HTTPDNS reuse must pass actual source, local pairing and consumed inherited
proofs; unchanged failed outcomes without a client retain their fallback. The same
candidate supplies comparison and construction in Controller and Gateway. Other
local runtime proofs and replay gates remain open; this is not a full runtime verdict.
Global setter rebuild equality deliberately includes complete selected Secret
metadata and raw reference Option distinctions. It always publishes current
provenance, even when effective values compare equal. Fingerprint equality remains
separate and excludes UID/resourceVersion; do not substitute it for setter equality.
Re-evaluate if a provider begins consuming
a different subset of TLS material or changes its rebuild policy.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-009`.
  This is distinct from the earlier, unrelated `ISSUE-009` in the LinkSys hot
  reload documentation review.

## Etcd candidate replay

EtcdCandidate also uses the canonical masked inherited-axis encoder in its
secret/material digest. Actual source, active ordered local pairing and consumed
inherited proofs precede RV/equality/zero-live planning. One candidate supplies
its cached fingerprint and captured policy to construction; retries cannot swap
in a later global snapshot. Absent/disabled local TLS consumes no inherited axes,
even when endpoint URLs use HTTPS. Config/credential field encoding remains
provider-specific. Authentication material reference authority is still a
separate outstanding consumer migration.

## OTLP candidate replay

OTLP also uses the canonical inherited-axis encoder in its secret/material
fingerprint after actual source, full provider config, whole active pairing and
consumed inherited proof. HTTPS absent/disabled local TLS hashes empty-local
inheritance; HTTP hashes no inherited policy. An unchanged resourceVersion does
not hide consumed global changes, while local axis overrides mask matching
global values and proof failures. One immutable candidate supplies the cached
fingerprint and captured snapshot to cold transport construction.
