#!/usr/bin/env bash

acquire_k8s_integration_lease() {
  local context="$1" namespace="$2" name="$3" holder="$4"
  local output existing_holder

  if ! [[ "${namespace}" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$ \
    && "${name}" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$ \
    && "${holder}" =~ ^[A-Za-z0-9]([A-Za-z0-9_.-]*[A-Za-z0-9])?$ ]]; then
    echo "invalid Kubernetes integration lease identity" >&2
    return 1
  fi

  output="$(
    jq -n --arg namespace "${namespace}" --arg name "${name}" --arg holder "${holder}" '{
      apiVersion: "coordination.k8s.io/v1",
      kind: "Lease",
      metadata: {namespace: $namespace, name: $name},
      spec: {holderIdentity: $holder}
    }' | kubectl --context "${context}" create -f - 2>&1
  )" && return 0

  existing_holder="$(
    kubectl --context "${context}" -n "${namespace}" get lease "${name}" \
      -o 'jsonpath={.spec.holderIdentity}' 2>/dev/null || true
  )"
  echo "Kubernetes integration cluster is already leased by ${existing_holder:-<unknown>}: ${output}" >&2
  return 1
}

release_k8s_integration_lease() {
  local context="$1" namespace="$2" name="$3" holder="$4"
  local existing_holder

  existing_holder="$(
    kubectl --context "${context}" -n "${namespace}" get lease "${name}" \
      --ignore-not-found -o 'jsonpath={.spec.holderIdentity}' 2>/dev/null
  )" || {
    echo "failed to read Kubernetes integration lease ${namespace}/${name}" >&2
    return 1
  }
  if [[ -z "${existing_holder}" ]]; then
    return 0
  fi
  if [[ "${existing_holder}" != "${holder}" ]]; then
    echo "refusing to release Kubernetes integration lease owned by ${existing_holder}" >&2
    return 1
  fi
  kubectl --context "${context}" -n "${namespace}" delete lease "${name}" --wait=true >/dev/null
}
