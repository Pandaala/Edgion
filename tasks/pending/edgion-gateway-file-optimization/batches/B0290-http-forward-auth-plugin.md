# B0290 — HTTP Forward Auth Plugin

- Pipeline state: analysis-ready; change candidate, complex / plan-first; no technical mutation authorized.
- Active anchor: `edgion-gateway`; baseline `60374898`.
- Target: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`.
- Analyst: `b0290_forward_auth_analysis`; full current target, external changes, call chains, original rules/knowledge and deterministic test/value coverage assigned. Prior B0077 evidence is historical; external 9b787978..b297af6e reopened the file.
- B0288 owns the serial lane; B0289 precedes this candidate. Refresh before promotion. Preserve external task resolutions and deduplicate existing findings.
- Explicit gates include canonical outbound client/TLS/egress/key extraction, body limits/streaming, failure policy, binary/access-log ownership, redaction/restraint/unification and English-only content. No edits, builds/tests/network, integration additions/edits or protected user-path changes.
- Full target/current consumers examined; blob `0ba941cd9c8082708ab560dd7abb861791c4c709`. `send_error_response` selects only HeaderMap::get and inserts one value, losing subsequent WWW-Authenticate/Set-Cookie values. B0077 preserved observed behavior, not an accepted exception to current repeated-response-header rule P.
- Proposed exact technical scope: target; new `skills/04-review/forward-auth/regression-guards/deny-response-repeated-headers-preserved.md`; owning forward-auth knowledge index. Iterate retained values using first insert/subsequent append without an owned value collection; duplicate/case-equivalent configured names must not multiply output. Preserve current text filtering, success-side identity behavior, body transfer, framing and write/shutdown order. Target's selective request example may also add the omitted forwardAll:false setting.
- Proposed mock assertions: separate ordered challenges and cookies, duplicate configured names, status/content-type/body ownership/EOS/shutdown/unselected-header behavior. Existing safe module filter selects seven mock/pure tests; no fresh tests executed.
- External presence diagnostics and D0078 resolution remain valid; D0037/D0038 closed identity tradeoffs and separate request-template/local-status work must not be reopened. D0153 records future real repeated-header integration coverage; current integration files stay untouched.
