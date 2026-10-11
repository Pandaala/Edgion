---
id: ARCH-201
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-182
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Borrow unchanged XML token text before constructing its owned node

The private decode function in edgion-dsl/src/xml.rs always builds a String through Writer. Parse then passes that String to XmlValue::text or XmlValue::attribute, whose owned helper validates and copies the decoded text into Arc<str>. The temporary String is discarded. Namespace declarations similarly materialize decoded String scratch before creating their retained Arc. These scratch values are operation-local decoding results, not independent runtime owners.

The allocation witness does not require a new workload. Gateway call_builtin_xml invokes the same leaf parser for both Str and request Body inputs. Its actual xml_body_scans_memory_and_spill_without_materializing_replay test contains 128 KiB of plain padding text. That token has no references or carriage returns, so decode emits exactly the borrowed token bytes, builds a complete intermediate String, and then copies those bytes into the immutable text node. Current source permits larger admitted tokens under the independent 1 MiB source and decoded bounds. This establishes avoidable token scratch construction and copying, not measured latency, RSS, deployment frequency or a total-operation memory figure.

Change only this private decoder/result seam to return a borrowed or owned string, such as Cow<'a, str>. Keep the existing character/reference walk as the canonical decoder. Retain a borrowed result while the emitted bytes are unchanged; when an entity or whitespace normalization first changes them, materialize the unchanged prefix once and continue through the existing bounded owned-output path. Parse consumes that result immediately. Final XmlValue nodes and namespace bindings remain independently owned Arc values, so neither a borrowed source slice nor the Body scanner's transient input escapes into the returned tree.

A no-transformation result must still perform the existing validation, decoded-length checks and work accounting. Do not replace the per-character sequence with an upfront total debit or early length rejection: those can change which error wins when a byte limit and remaining work compete. Preserve the current sequence of emitted-byte precharge, output-byte admission and append charge, including the budget remaining after failure. A late transition to owned output must not charge the already-accounted prefix a second time, and must retain the existing capped reservation bound. Conservative current charges may remain even when borrowing eliminates a scratch allocation; this recommendation does not weaken the nonrefundable work ledger.

Preserve text versus attribute normalization, CRLF consumption, numeric-reference grammar, XML character checks, reference-produced whitespace, CDATA's disabled reference decoding, and every typed limit/Invalid/Work outcome. Leave namespace resolution, lexical and expanded duplicate-attribute checks, constructor validation, expanded summaries, identity equality, serialization and its declaration bound unchanged. Final String-to-Arc conversion remains a real copy where required; no Arc representation migration or zero-copy conversion claim is proposed.

The improvement is confined to private leaf parsing. It introduces no XML regex, second parser, runtime dependency, public decoder framework, source-backed XML node, request cache, response aggregation, new observability or mock convenience allocation. Gateway Body admission, scan reservation/settlement, blocking-lane ownership, cancellation, fixed BodyAccess failures and one-shot overwrite remain their current owners.

The original bounded-XML and sandbox documents require bounded producers and exact semantic/work boundaries, but do not require allocating unchanged decoded scratch. The existing SSO deferral concerns changing Value::Str and its broad allocation tradeoffs; this proposal retains that representation. ARCH-054 concerns a separate transcoder percent-decoder, not this XML owner. Scoped review/task searches found no existing XML scratch-ownership finding. The XML feature task records implemented acceptance, but its linked xml/01-design.md ledger is absent from the current tree; historical acceptance is not fresh execution or authority to alter current charging semantics.

All leaf XML and Gateway XML builtin test bodies were read. Future verification should retain those cases and add focused private-decoder coverage for unchanged text/CDATA/attributes, early and late transformations, numeric-reference whitespace, multibyte text, exact byte/work admission and competing-error ordering. Source inspection must establish no decoded String construction on the unchanged branch and no borrowed data in returned nodes. The value proposition is one avoided intermediate payload per unchanged token; no benchmark result or broader cache/parser redesign is claimed.
