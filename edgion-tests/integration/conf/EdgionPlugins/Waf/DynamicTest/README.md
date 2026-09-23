# Native WAF hot reload

This scenario proves initial publication, a valid generation swap, retention of
the last valid generation after a non-final WAF deny status, recovery with a
later valid generation, deletion fail-close behavior for the dangling ExtensionRef, and
re-creation. The initial phase also proves a phase-2 body denial before the
request reaches the backend. Its phase-3 rule sends a final 403 after the
backend responds; the wire test proves that the Gateway replaces the
backend's original 200 response.
