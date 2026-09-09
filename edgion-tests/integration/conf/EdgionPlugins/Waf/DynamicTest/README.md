# Native WAF hot reload

This scenario proves initial publication, a valid generation swap, retention of
the last valid generation after invalid SecLang, recovery with a later valid
generation, deletion fail-close behavior for the dangling ExtensionRef, and
re-creation. The initial phase also proves a phase-2 body denial before the
request reaches the backend. Its path-isolated phase-3 rule stages an invalid
informational local-reply status after the backend responds; the wire test
proves that the Gateway fails closed with 500 instead of forwarding the
backend's original 200 response.
