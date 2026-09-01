# Native WAF hot reload

This scenario proves initial publication, a valid generation swap, retention of
the last valid generation after invalid SecLang, recovery with a later valid
generation, deletion fail-close behavior for the dangling ExtensionRef, and
re-creation. The initial phase also proves a phase-2 body denial before the
request reaches the backend.

