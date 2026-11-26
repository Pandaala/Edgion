# Request body parser outcome in SecLang (`REQBODY_ERROR`)

Full inspection exposes one read-only scalar from phase 2 onwards. A successful body publication yields ASCII 0. A completed fallback publication with a typed syntax or multipart boundary error yields ASCII 1. Each new transaction starts without an error. Phase 1 has no completed inspection result. Named selectors and request-body prefix configuration are rejected for this target.

Coraza initializes a per-transaction scalar, sets it on processor failure and evaluates phase 2. Its recommended rule 200002 uses the result to deny with status 400. ModSecurity v3 tracks tolerated percent-decoding errors separately. Edgion implements the rule predicate with its existing Gateway default and error classifications. See [Coraza processing](https://github.com/corazawaf/coraza/blob/main/internal/corazawaf/transaction.go), [recommended policy](https://github.com/corazawaf/coraza/blob/main/coraza.conf-recommended) and [ModSecurity processing](https://github.com/owasp-modsecurity/ModSecurity/blob/v3/master/src/transaction.cc).

| Input and inspection | Phase 2 result |
| --- | --- |
| Complete JSON or XML, snapshot or stream | Successful parsing yields 0. A definitive syntax failure latches during parsing or at real EOF. On complete fallback publication, abandon provisional structured results and expose 1. |
| Complete multipart | Successful parsing yields 0. Malformed structure, missing boundary, multiple Content-Disposition headers in a part, or conflicting name or filename parameters yield 1. Discard partial form and file entries. Separate parts may use the same name. |
| RAW or tolerantly decoded URL-encoded form | Publish 0 on success, including tolerated percent escapes. |
| Full policy cutoff | Preserve the existing priority of an already definitive parser failure. A latched JSON syntax failure checks all source bytes already observed against the lexical source limit on either completion path. Otherwise reject at the configured cutoff or skip whole body-dependent units under Continue. |
| Independent limits, storage or transport failures | Preserve existing typed failures and status mapping. |

Compile the dependency from effective rule targets, including chain members and later phases. A whole-scalar exclusion removes earlier selections of the flag, including counts. A later positive selection restores the dependency. Other request-body targets retain their own dependencies. A flag-only rule requests complete processing. Generations without consumers retain early enforcing failures. Reuse the transaction's typed error and existing fallback visibility, accounting and request storage. Run phase 2 once after complete publication. Parser diagnostics and partial structured views are outside the public variable contract.

| Fully received malformed request | On | DetectionOnly |
| --- | --- | --- |
| No intervention, including scoring-only | 400, zero upstream dispatches | Forward once, upstream supplies status |
| Rule denies with status S | S, zero upstream dispatches | Record match and forward once |

The no-consumer default applies wherever the current configuration inspects the body. This feature adds no global parsing requirement. Tests cover a flag-only rule, consecutive transactions, buffered and streamed parsing, cutoff, limits and gateway dispatch decisions. Companion variables, parser messages and additional macros require separate design.
