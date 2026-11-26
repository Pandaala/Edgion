---
name: cli-controller-diagnostics
description: Review Controller diagnostic report selection, authentication, output and exit status in the CLI.
knowledge_type: regression-guards
---

# Controller diagnostics in the CLI

For common-05, the CLI gained read-only access to the Controller's existing file conflict
and unknown-field reports. The CLI uses the center target and its normal authenticated
HTTP client. A successful query prints the API envelope, including empty conflict lists
and the unknown-field report's total and truncation flag. Findings are query results.
An unsuccessful query must exit nonzero without printing a report on stdout.
Validate the selected report's required fields and preserve additional fields.
Strip the request URL from response-body errors. A standby's HTTP 409 can
identify the leader by host and port or an explicitly supplied HTTP(S) origin
for a manual retry. Closing the output pipe
early ends a successful query quietly. For unknown fields, `truncated` describes
the resource list; each resource's paths have independent display limits.

The Controller owns the findings in `api/diagnostics_handlers.rs`. Recheck the CLI
mapping and its [command-line tests](../../../../edgion-cli/tests/diagnostics.rs) if
the routes, response envelope or truncation contract change. Fleet sync and other
[deferred CLI work](../../../../tasks/todo/cli-operational-inspection.md) have separate acceptance criteria.
