#!/usr/bin/env bash
set -euo pipefail

readonly NODE_IMAGE='docker.io/kindest/node:v1.26.15@sha256:c79602a44b4056d7e48dc20f7504350f1e87530fe953428b792def00bc1076dd'
readonly CRD='config/crd/edgion-crd/edgion_backend_crd.yaml'
readonly CLUSTER_NAME="edgion-backend-k126-${GITHUB_RUN_ID:-local}-$$"
readonly CONTEXT="kind-${CLUSTER_NAME}"

for tool in docker kind kubectl python3; do
    command -v "${tool}" >/dev/null || {
        echo "required tool is unavailable: ${tool}" >&2
        exit 1
    }
done

cleanup() {
    kind delete cluster --name "${CLUSTER_NAME}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

kind create cluster --name "${CLUSTER_NAME}" --image "${NODE_IMAGE}" --wait 120s

server_version="$({ kubectl --context "${CONTEXT}" version -o json; } | python3 -c \
    'import json,sys; print(json.load(sys.stdin)["serverVersion"]["gitVersion"])')"
if [[ "${server_version}" != 'v1.26.15' ]]; then
    echo "expected Kubernetes v1.26.15, got ${server_version}" >&2
    exit 1
fi

kubectl --context "${CONTEXT}" apply -f "${CRD}"
kubectl --context "${CONTEXT}" wait \
    --for=condition=Established \
    --timeout=120s \
    crd/edgionbackends.edgion.io

resource_json() {
    local case_name="$1"
    python3 - "${case_name}" <<'PY'
import json
import sys

case = sys.argv[1]
obj = {
    "apiVersion": "edgion.io/v1",
    "kind": "EdgionBackend",
    "metadata": {"name": "provider", "namespace": "default"},
    "spec": {
        "ai": {
            "provider": "OpenAI",
            "credentialPool": {
                "credentials": [{"name": "primary", "secretRef": {"name": "provider-key"}}]
            },
            "models": [{"name": "model-a"}],
        }
    },
}
ai = obj["spec"]["ai"]
credential = ai["credentialPool"]["credentials"][0]

if case == "valid":
    pass
elif case == "missing_spec":
    del obj["spec"]
elif case == "empty_spec":
    obj["spec"] = {}
elif case == "unknown_spec":
    obj["spec"]["a2a"] = {}
elif case == "unknown_ai":
    ai["unknownField"] = True
elif case == "unknown_pool":
    ai["credentialPool"]["unknownField"] = True
elif case == "unknown_credential":
    credential["unknownField"] = True
elif case == "unknown_secret_ref":
    credential["secretRef"]["unknownField"] = True
elif case == "unknown_limits":
    credential["limits"] = {"requestsPerMinute": 1, "unknownField": True}
    ai["credentialPool"]["redisRef"] = "default/quota"
elif case == "unknown_model":
    ai["models"][0]["unknownField"] = True
elif case == "unknown_pricing":
    ai["models"][0]["pricing"] = {
        "inputPerMillionUsd": "1",
        "outputPerMillionUsd": "2",
        "unknownField": True,
    }
elif case == "unknown_defaults":
    ai["defaults"] = {"unknownField": True}
elif case == "missing_provider":
    del ai["provider"]
elif case == "invalid_provider":
    ai["provider"] = "Other"
elif case == "missing_compatible_endpoint":
    ai["provider"] = "OpenAICompatible"
elif case == "invalid_endpoint":
    ai["endpoint"] = "https://user@example.com/v1?query=yes"
elif case == "insecure_endpoint":
    ai["endpoint"] = "http://example.com/v1"
elif case == "empty_credentials":
    ai["credentialPool"]["credentials"] = []
elif case == "too_many_credentials":
    ai["credentialPool"]["credentials"] = [
        {"name": f"credential-{i}", "secretRef": {"name": "provider-key"}} for i in range(17)
    ]
elif case == "invalid_alias":
    credential["name"] = "bad alias"
elif case == "invalid_weight":
    credential["weight"] = 0
elif case == "missing_secret_name":
    del credential["secretRef"]["name"]
elif case == "empty_secret_name":
    credential["secretRef"]["name"] = ""
elif case == "invalid_secret_group":
    credential["secretRef"]["group"] = "external.example"
elif case == "invalid_secret_kind":
    credential["secretRef"]["kind"] = "ExternalSecret"
elif case == "invalid_secret_namespace":
    credential["secretRef"]["namespace"] = "Invalid_Namespace"
elif case == "empty_limits":
    credential["limits"] = {}
    ai["credentialPool"]["redisRef"] = "default/quota"
elif case == "invalid_limit":
    credential["limits"] = {"requestsPerMinute": 0}
    ai["credentialPool"]["redisRef"] = "default/quota"
elif case == "limit_without_redis":
    credential["limits"] = {"requestsPerMinute": 1}
elif case == "redis_without_limit":
    ai["credentialPool"]["redisRef"] = "default/quota"
elif case == "invalid_redis_ref":
    ai["credentialPool"]["redisRef"] = "invalid"
elif case == "tpm_without_max_output":
    credential["limits"] = {"tokensPerMinute": 1}
    ai["credentialPool"]["redisRef"] = "default/quota"
elif case == "empty_models":
    ai["models"] = []
elif case == "too_many_models":
    ai["models"] = [{"name": f"model-{i}"} for i in range(257)]
elif case == "empty_model_name":
    ai["models"][0]["name"] = ""
elif case == "too_many_aliases":
    ai["models"][0]["aliases"] = [f"alias-{i}" for i in range(33)]
elif case == "invalid_pricing":
    ai["models"][0]["pricing"] = {"inputPerMillionUsd": "1.0000001", "outputPerMillionUsd": "2"}
elif case == "missing_price":
    ai["models"][0]["pricing"] = {"inputPerMillionUsd": "1"}
elif case == "invalid_max_output":
    ai["defaults"] = {"maxOutputTokens": 0}
else:
    raise SystemExit(f"unknown test case: {case}")

print(json.dumps(obj))
PY
}

apply_dry_run() {
    kubectl --context "${CONTEXT}" create \
        --dry-run=server \
        --validate=strict \
        -f -
}

resource_json valid | apply_dry_run >/dev/null

expect_rejected() {
    local case_name="$1"
    local expected="$2"
    local output
    local status
    set +e
    output="$(resource_json "${case_name}" | apply_dry_run 2>&1)"
    status=$?
    set -e
    if [[ ${status} -eq 0 ]]; then
        echo "Kubernetes accepted invalid EdgionBackend case: ${case_name}" >&2
        exit 1
    fi
    if [[ "${output}" != *"${expected}"* ]]; then
        echo "case ${case_name} did not return expected cause ${expected}" >&2
        echo "${output}" >&2
        exit 1
    fi
}

expect_rejected missing_spec 'spec: Required value'
expect_rejected empty_spec 'spec must configure exactly one backend variant'

for case_name in \
    unknown_spec unknown_ai unknown_pool unknown_credential unknown_secret_ref \
    unknown_limits unknown_model unknown_pricing unknown_defaults; do
    expect_rejected "${case_name}" 'unknown field'
done

for case_name in \
    missing_provider invalid_provider missing_compatible_endpoint invalid_endpoint insecure_endpoint \
    empty_credentials too_many_credentials invalid_alias \
    invalid_weight missing_secret_name empty_secret_name invalid_secret_group \
    invalid_secret_kind invalid_secret_namespace empty_limits invalid_limit \
    limit_without_redis redis_without_limit invalid_redis_ref tpm_without_max_output \
    empty_models too_many_models empty_model_name too_many_aliases invalid_pricing \
    missing_price invalid_max_output; do
    expect_rejected "${case_name}" 'EdgionBackend'
done

resource_json valid | kubectl --context "${CONTEXT}" create --validate=strict -f - >/dev/null
stored_provider="$(kubectl --context "${CONTEXT}" get edgionbackend provider -o jsonpath='{.spec.ai.provider}')"
if [[ "${stored_provider}" != 'OpenAI' ]]; then
    echo "created EdgionBackend did not retain the valid nested provider: ${stored_provider}" >&2
    exit 1
fi

if ! kubectl --context "${CONTEXT}" api-resources --api-group=edgion.io -o name | \
    grep -qx 'edgionbackends.edgion.io'; then
    echo 'EdgionBackend is absent from API discovery' >&2
    exit 1
fi

kubectl --context "${CONTEXT}" explain \
    edgionbackend.spec.ai.credentialPool.credentials.secretRef >/dev/null

echo 'EdgionBackend Kubernetes v1.26.15 admission contract passed'
