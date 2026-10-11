---
id: ARCH-100
status: candidate
kind: behavior-defect
priority: P2
partition: PART-403
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-100: Reject invalid LabelSelector expression cardinality before namespace matching

## Current ownership and supported witness

NamespaceStore supplies committed Namespace labels to the existing label_selector_matches evaluator. In namespace_store.rs:116-140, evaluate_expression converts absent values into an empty vector and evaluates NotIn using labels.get(key).is_none_or(...). An absent or empty values array therefore makes NotIn match every label set.

A Gateway listener can select namespaces using:

{"matchExpressions":[{"key":"team","operator":"NotIn"}]}

The same problem occurs with "values": []. With a committed route Namespace containing team=restricted, the evaluator returns true. For an otherwise eligible cross-namespace parent reference, route_utils::listener_allows_route_namespace accepts that Namespace and validate_parent_ref_accepted uses the result in listener acceptance.

This is a supported typed path: RouteNamespaces.selector is retained as serde_json::Value, the Kubernetes Namespace watcher commits actual Namespace labels, and GatewayHandler::validate does not validate expression cardinality. The vendored standard Gateway API v1.6.2 Gateway v1 and v1beta1 schemas require key and operator but do not impose a values minimum or operator-dependent CEL check. Their descriptions explicitly require nonempty values for In/NotIn and empty values for Exists/DoesNotExist. No live API-server or traffic execution is claimed.

## Bounded improvement

Keep the existing namespace store and canonical local evaluator. Validate expression values at the evaluate_expression boundary before matching:

- In and NotIn require a nonempty array of strings.
- Exists and DoesNotExist permit absent values or an empty array, and reject nonempty values.
- Invalid values shapes return false rather than being reduced to a valid-looking empty list.

Continue matching valid expressions with borrowed input. Do not introduce a selector framework, another Namespace cache, new I/O, metrics, or logs.

The pinned kube-core 2.0.1 Selector conversion is not a sufficient replacement: its TryFrom<LabelSelectorRequirement> implementation rejects absent values for In/NotIn but accepts Some(empty), and it ignores values for Exists/DoesNotExist. Native conversion alone would leave the decisive cardinality defect.

## Required semantics and scope

Preserve valid NotIn behavior when the label key is absent, valid In/Exists/DoesNotExist behavior, AND composition, valid empty-object selectors, existing unknown-operator rejection, and existing Namespace-label refresh and missing-label fallback behavior.

This candidate does not redefine the evaluator's separate top-level null behavior or route_utils' separate missing-selector branch. It does not change All/Same handling or introduce a new routing owner. It concerns expression cardinality in the current evaluator.

## Original knowledge and deduplication

The Gateway architecture, feature reference and vendored Gateway API schema establish namespace-selection semantics. Current source and tests contain valid expression coverage but no rejection coverage for these invalid cardinalities. No existing ARCH candidate found in the review artifacts owns this expression-validation cause.

## Future verification

Add evaluator regressions for absent and empty NotIn values with both present and absent label keys, absent/empty In values, and nonempty values on Exists/DoesNotExist. Preserve controls for all valid operators, AND composition and empty selectors.

Add a focused namespace-gate test using the existing real Gateway/route fixtures and committed Namespace labels: the invalid NotIn selector must reject the cross-namespace route. No network environment or widened production visibility is needed.
