---
name: client-certificate-expiry-on-resumption
description: Use when reviewing downstream mTLS session lifetime across client leaf certificate expiry.
knowledge_type: intentional-design
---

# Client certificate expiry after a full handshake

Edgion accepts the client authentication from a successful full handshake when
a session resumes under the same effective downstream authentication policy.
The client leaf certificate can expire before that session stops being resumable.
This preserves abbreviated handshakes for authenticated clients. A subsequent
full handshake still subjects the presented certificate to normal verification.

`apply_selected_downstream_policy` installs the selected session ID context.
BoringSSL 5.4 checks the session lifetime in `ssl_session_is_resumable`;
it does not compare the peer certificate's `notAfter` to the resumption time.
TLS 1.3 renewal remains bounded by the original authentication lifetime.
This conclusion is based on source inspection.

Revisit if Edgion requires a valid client certificate at every reconnection.
Disabling authenticated resumption would require a full handshake every time.
Capping each authenticated session at the leaf expiry would retain earlier
resumption but requires enforcement for TLS 1.2 tickets and cached sessions
as well as TLS 1.3 ticket renewal. A fixed timeout alone cannot enforce expiry.

This resolves the client leaf expiry question from gw-infra-01 follow-up item 2.
Revocation policy is outside this decision. Cross-policy isolation remains
covered by the [session context regression guard](../regression-guards/session-resumption-policy-isolation.md).
