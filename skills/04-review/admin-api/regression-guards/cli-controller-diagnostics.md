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

For the common-05 server inspection follow-up, the resource-list HTTP 5xx hint
must name a working command. The top-level `server-info` command uses the default
center target and the Controller's separate `server-info` permission. A valid
response reports the instance ID, mode and readiness, including an empty ID and
`ready: false` during initialization. Query failures exit nonzero with empty
stdout. The CLI process tests cover the endpoint, authentication and both readiness states.

The Controller owns the findings in `api/diagnostics_handlers.rs`. Recheck the CLI
mapping and its [command-line tests](../../../../edgion-cli/tests/diagnostics.rs) if
the routes, response envelope or truncation contract change. Separate common-05 tasks cover
[route status](../../../../tasks/todo/cli-route-status.md),
[fleet sync](../../../../tasks/todo/cli-fleet-sync-status.md),
[source diff](../../../../tasks/todo/cli-config-diff.md) and
[target names](../../../../tasks/todo/cli-target-names.md).
