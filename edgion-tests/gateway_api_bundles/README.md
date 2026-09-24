# Gateway API release fixture evidence

`artifacts.json` locks the SHA-256 of each official Gateway API GitHub release
`standard-install.yaml` and `experimental-install.yaml` artifact downloaded
for the exact stable releases v1.0.0, v1.1.0, v1.1.1, v1.2.0, v1.2.1,
v1.3.0, v1.4.0, v1.4.1, v1.5.0, v1.5.1, v1.6.0, v1.6.1, and v1.6.2.
Artifacts are cached under `/tmp/edgion-gateway-api-bundles`; no historical
schema is placed in the production `config/crd` directory.

The locked checks were executed on 2026-09-24: 26 release artifacts,
338 source rows, 266 minimal OpenAPI fixture passes, 72 N/A rows, and zero
failures. Standard contributed 120 passes/49 N/A; Experimental contributed
146 passes/23 N/A.

Run the locked check with Python 3 and the versions in `requirements.txt`
installed:

```bash
python3 -m venv /tmp/edgion-gateway-api-fixture-venv
/tmp/edgion-gateway-api-fixture-venv/bin/python -m pip install -r edgion-tests/gateway_api_bundles/requirements.txt
/tmp/edgion-gateway-api-fixture-venv/bin/python edgion-tests/gateway_api_bundles/check.py
cargo test -p edgion-resources --test gateway_api_bundle_fixtures
```

For the 2026-09-24 run, dependencies were installed under
`/tmp/edgion-batch5-python` and the command used
`PYTHONPATH=/tmp/edgion-batch5-python python3 .../check.py`.

The Python runner checks one shared minimal `fixtures.json` document for each
inventoried source GVK against the OpenAPI v3 schema in each exact release and
channel. `openapi-results.tsv` says `N/A` when that bundle does not serve the
source GVK. `field-path-results.tsv` records exact HTTPRoute v1 field presence
by release, channel, and path. `drift-results.tsv` exercises four exact
Standard release pairs with positive and negative fixtures: HTTPRoute v1
`spec.rules[].matches` 9 entries (v1.1.1 fails, v1.2.0 passes), Gateway v1
`spec.addresses[].value` omitted (v1.2.1 fails, v1.3.0 passes), TLSRoute v1
`spec.hostnames` 17 entries (v1.5.1 fails, v1.6.0 passes), and
BackendTLSPolicy v1 `spec.validation.wellKnownCACertificates` using a
non-System name (v1.4.1 fails, v1.5.0 passes).
`schema-change-results.tsv` records exact before/after values for the added
HTTPRoute parentRef port, new RequestMirror fraction denominator default,
and corrected Gateway listener protocol regex. The regex correction removes
a redundant `S` from its character class; its accepted language is unchanged,
so no discriminating before/after instance exists for that correction.
`corrected-validation-results.tsv` checks representative accepted `HTTP` and
`example.com/Proto` protocols and rejected `_` against both exact releases.
These checks do not execute CRD CEL, Kubernetes
defaulting/pruning/conversion, Edgion Controller semantic validation, or
cluster conformance. A passing minimal fixture does not prove compatibility
with every field in that source GVK.

For example, the exact path rows show that
`spec.rules[].sessionPersistence` first appears in the Experimental
HTTPRoute/v1 fixture at v1.1.0, `spec.rules[].retry` at v1.2.0,
`spec.rules[].filters[].cors` at v1.3.0 (and in Standard at v1.5.0),
and `spec.useDefaultGateways` plus both rule/backend-filter
`externalAuth` paths at v1.4.0. All later exact tested releases have the
same presence result for these paths; consult the TSV for each release.

The Rust test independently decodes and preparses the nine canonical source
models (`GatewayClass`, `Gateway`, `HTTPRoute`, `GRPCRoute`, `TLSRoute`,
`ReferenceGrant`, `TCPRoute`, `UDPRoute`, `BackendTLSPolicy`) with the shared
fixtures. It also decodes and converts the four implemented v1beta1 adapters
(`GatewayClass`, `Gateway`, `HTTPRoute`, `ReferenceGrant`) into canonical models.
Those checks execute the Edgion source model boundary, but they do not run an
end-to-end Controller, Gateway, or per-release cluster path. The Rust test is
one shared fixture pass, not a separate execution against each downloaded
release artifact.
Additional Rust fixtures retain the added HTTPRoute parentRef port, decode
nine matches and 17 TLSRoute hostnames, and check that Edgion's RequestMirror
validation uses the absent denominator's default of 100. Controller tests
check the unsupported Gateway address and named CA status/withdrawal paths;
Gateway List tests check mixed sibling objects and repair. None of these
proves all accepted instances under the changed schema.

The matrix is deliberately narrow. Same-GVK schema drift beyond the checked
minimal documents, four differential instances, and enumerated field paths
remains unverified. The newly optional Gateway address value is accepted by
the Rust source model, then rejected per object by the Controller with
`Accepted=False` and no listener publication because Edgion cannot bind an
address lacking a value. That is a safe unsupported-field outcome, not
runtime support for an unspecified address. No row
in this directory is a per-source conformance claim.
