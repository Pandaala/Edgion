# Guardrail integration fixture

This focused suite reuses the complete `EdgionBackend/SingleProvider` AI
Gateway, provider, and credential fixtures. The shared `test_server` also acts
as the unary Guardrail decision service. The suite covers byte-identical Pass,
semantic Replace, a wire-valid policy refusal with `failOpen: true`, and
malformed-decision fail-open. The head-hold fixture proves that a first-window
Reject replaces the uncommitted origin response with complete `403` JSON on
both HTTP/1.1 and HTTP/2. It also covers a static deadline fail-close and
Guardrail followed by the existing ExtProc response mutator. Content-free fixture observations
prove the minimal request envelope and exactly one call for each canonical
window, including a two-window stream. The Gateway access-log store verifies
one aggregate Guardrail summary and no per-window log record. No additional
process, listener, provider snapshot, or port is required.
