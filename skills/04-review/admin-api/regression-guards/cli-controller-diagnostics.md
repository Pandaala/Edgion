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
the routes, response envelope or truncation contract change.

For the common-05 source comparison (`cli-config-diff`), the center resource GET
returns the persisted document directly. Validate its identity and compare the
structured content before rendering only document ordinals and fixed categories.
Rendering arbitrary paths can expose credentials in keys and annotations. A
failed read or parse must suppress the batch and exit 2, distinct from a completed
comparison with differences (1) or equality (0). Recheck the
[CLI process tests](../../../../edgion-cli/tests/config_diff.rs) if the source GET,
normalization, output, or failure contract changes. Only a confirmed named-object
absence carries the source GET's missing-object header. Kubernetes discovery
failure or a generic upstream 404 must never acquire that header. Match string
keys and round-trip decimals across the YAML source and JSON response, reject
duplicate keys and explicit null documents, and keep missing-token guidance
distinct from Controller initialization.

Separate common-05 tasks cover
[route status](../../../../tasks/todo/cli-route-status.md),
[fleet sync](../../../../tasks/todo/cli-fleet-sync-status.md) and
[target names](../../../../tasks/todo/cli-target-names.md).
