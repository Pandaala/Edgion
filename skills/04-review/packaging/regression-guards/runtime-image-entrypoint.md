---
name: runtime-image-entrypoint
description: Use when reviewing the Dockerfile.runtime ENTRYPOINT/CMD contract for released Gateway, Controller, and test-harness images.
knowledge_type: regression-guards
---

# Runtime Image Entrypoint Is Fixed at Build Time

## Conclusion and scope

`cicd/docker/Dockerfile.runtime` (release workflow, `cicd/build-image.sh`, OrbStack
fast builds) selects its binary through the build-time `BINARY`/`BUILD_TYPE`
arguments. The selection must be materialized during image construction: the copy
step generates `/usr/local/bin/edgion-entrypoint`, a two-line wrapper that runs
`exec /usr/local/bin/<selected> "$@"`, where `<selected>` is `edgion-${BINARY}`
(`bin`) or `${BINARY}` (`example`). The image uses exec-form
`ENTRYPOINT ["/usr/local/bin/edgion-entrypoint"]` with no `CMD`.

Required behavior: the default `docker run <image>` starts the selected binary as
PID 1 under its own process name; argument-only invocation
(`docker run <image> --config-file ...`, or Kubernetes `args` without `command`)
forwards those arguments to it; signals reach it directly and its exit status is the
container exit status. Arguments after the image are binary flags, not a command
name. Explicit `command` overrides are unaffected.

Args-only consumers include `cicd/deploy/kubernetes/{controller,gateway}/deployment.yaml`,
gateway Deployments derived from them by `provision-instance.py`, and the test-server
bootstrap manifest `edgion-tests/k8s/conf/01-deployment.yaml`. Most other
`edgion-tests/k8s/kubernetes` manifests set `command` explicitly.

## Rationale and evidence

The previous form was `ENTRYPOINT ["/bin/sh", "-c"]` plus
`CMD ["exec /usr/local/bin/edgion-${BINARY}"]`. `ARG` values are not runtime
environment variables, and JSON-form `CMD` is not expanded at build time, so the
runtime shell expanded `${BINARY}` to empty and tried `/usr/local/bin/edgion-`.
Kubernetes `args` also replaced `CMD`, making `sh -c` treat `--config-file` as the
script and never reach the binary. Released images therefore failed to start with
both the default invocation and the shipped manifests. The integration runs that
exercise Gateway/Controller use `command` overrides, so they did not surface it.

## Alternatives and limits

- Adding only `ENV BINARY` fixes the default invocation but still breaks
  argument-only invocation through `sh -c`.
- A symlink entrypoint was rejected: `execve` through a symlink names the process
  `edgion-entrypoint` in `/proc/<pid>/comm`, hiding it from `ps`/`pgrep edgion-gateway`.
  The wrapper costs one extra `exec` at startup and leaves no shell running.
- The source-build `cicd/docker/Dockerfile` has a similar late-expansion shape inside
  its development restart loop; it is outside this guard.

## Verification and re-evaluation

2026-11-05: built the modified Dockerfile with stub binaries for `gateway`,
`controller` (`bin`) and `test_server` (`example`). Each image reported
`Entrypoint=["/usr/local/bin/edgion-entrypoint"]`, `Cmd=null`; the stub ran as PID 1
with its real name and forwarded `--config-file` arguments, received SIGTERM from
`docker stop`, and a non-zero exit (7) propagated as the container exit code. No real
product image was built. Re-evaluate if the entrypoint returns to a shell `-c` form,
`BINARY` selection moves to runtime, or the wrapper stops using `exec`.

## Resolution references

- ISSUE-012 (`tasks/important_issue/run-e6cbd068`, baseline `e6cbd0688eb0`),
  confirmed P1, fixed 2026-11-05. The same change corrected the
  `docker run ... edgion-controller --config-file` example in
  `docs/{en,zh-CN}/getting-started/installation-and-deployment.md`.
