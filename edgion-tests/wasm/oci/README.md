# Manual E2E: Wasm `source.oci` registry pull

This is a manual companion to the automated Wasm plugin suite
(`edgion-tests/integration/conf/EdgionPlugins/Wasm/01_EdgionPlugins_wasm.yaml`),
which only exercises `source.file`. There is deliberately no automated
integration test that pulls from a real OCI registry (spinning up
`registry:2` is out of scope for the fast local-process harness), so this
procedure is how a human verifies `source.oci` end to end: start a local
registry, push the reference guest module both ways an OCI wasm image can be
packaged, point an `EdgionPlugins` `Wasm` plugin at it, and confirm the
gateway pulls, caches, and (for `pullPolicy: Always`) reloads correctly.

`push_wasm_image.py` in this directory is a small stdlib-only Python script
(no `docker` CLI, no OCI client library) that pushes a `.wasm` file to a
registry as a minimal OCI image, in either of the two layer shapes the
gateway's OCI puller understands:

- **`oci` variant** — a single raw-wasm layer,
  `application/vnd.module.wasm.content.layer.v1+wasm` (matches
  `MEDIA_TYPE_WASM_LAYER` in
  `edgion-gateway/src/plugins/http/wasm/oci/manifest.rs`).
- **`compat` variant** — a single `tar.gz` layer,
  `application/vnd.oci.image.layer.v1.tar+gzip`, containing one file named
  `plugin.wasm` (the shape Istio/`WasmPlugin`-style images use).

## Reference facts (from the implementation)

- Default `pullPolicy` when unset: `Always` for a `latest` tag or no tag,
  `IfNotPresent` for any other tag. A digest-pinned ref (`@sha256:...`) is
  **always** treated as pinned regardless of `pullPolicy`
  (`WasmOciSpec::effective_pull_policy`).
- A digest-pinned ref never re-pulls: the manifest is loaded disk-cache-first
  by content digest, and so is the blob. On a cold gateway process (e.g.
  after a restart) with a warm disk cache, a pinned plugin loads with **zero**
  registry traffic.
- `pullPolicy: Always` re-checks the tag's manifest digest against the
  registry at most once every 60 seconds (`OCI_ALWAYS_RECHECK_SECS` in
  `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`); an unchanged digest
  keeps the already-loaded module, a changed digest reloads it in place.
- The disk cache directory defaults to `wasm-oci-cache/` under the gateway
  work directory (`gateway.wasm_oci_cache_dir` in `edgion-gateway.yaml`,
  unset = `wasm-oci-cache`). Entries are content-addressed
  (`<sha256-hex>.manifest.json` / `<sha256-hex>.blob`).
- The gateway treats a loopback registry host (`localhost`, `127.0.0.0/8`,
  `::1`) as plain HTTP — no TLS setup is needed to test against a local
  `registry:2`. Non-loopback registries require `https://` with a verifiable
  certificate.

## Prerequisites

- `docker` (to run `registry:2`).
- `python3` — the push script uses only the standard library.
- A running Edgion controller + gateway, either the local file-system
  config-center harness
  (`edgion-tests/integration/scripts/utils/start_all_with_conf.sh`) or a
  Kubernetes deployment (see
  `docs/en/getting-started/first-gateway.md`). This walkthrough uses
  `kubectl apply` phrasing; if you are on the file-system harness, drop the
  same YAML documents into the watched `conf_dir` (or `edgion-cli apply`)
  instead.
- `kubectl` only if you want to generate the pull-secret manifest for the
  private-registry variant (Step 6) — it is used purely to render YAML with
  `--dry-run=client`, no live cluster call is required for that step.

## Step 1: Start a local registry

```bash
docker run -d --rm -p 5000:5000 --name edgion-oci-e2e registry:2
```

Confirm it is up:

```bash
curl -s http://localhost:5000/v2/
# {}
```

## Step 2: Push the reference wasm module (both variants)

From the repo root:

```bash
# oci variant — raw wasm layer
python3 edgion-tests/wasm/oci/push_wasm_image.py \
  --registry localhost:5000 \
  --repo wasm/header-demo \
  --tag 1.0.0 \
  --wasm edgion-tests/wasm/artifacts/header_demo.wasm \
  --variant oci

# compat variant — tar.gz layer containing plugin.wasm
python3 edgion-tests/wasm/oci/push_wasm_image.py \
  --registry localhost:5000 \
  --repo wasm/header-demo-compat \
  --tag 1.0.0 \
  --wasm edgion-tests/wasm/artifacts/header_demo.wasm \
  --variant compat
```

Each run prints two lines:

```
pushed: localhost:5000/wasm/header-demo:1.0.0
pinned: localhost:5000/wasm/header-demo@sha256:<manifest-digest>
```

Keep the `pinned:` digest around — it is the exact `@sha256:...` reference
used in Step 3's digest-pinned example.

## Step 3: Point an `EdgionPlugins` `Wasm` plugin at the image

This mirrors
`edgion-tests/integration/conf/EdgionPlugins/Wasm/01_EdgionPlugins_wasm.yaml`
(same request/response phase wiring, same behavior contract — see that
file's header comment for the full `x-wasm-*` header contract), swapping
`source.file` for `source.oci`. No CRD change is required for either form:
`source.oci` rides the `EdgionPlugins` CRD's
`x-kubernetes-preserve-unknown-fields` plugin `config` schema.

**Tag form** (uses the default pull policy — `IfNotPresent` since `1.0.0`
is not `latest`):

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-oci-demo
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      enable: true
      config:
        source:
          oci:
            image: localhost:5000/wasm/header-demo:1.0.0
        pluginConfig:
          json: {}
        failOpen: false
        timeoutMs: 500
  upstreamResponseFilterPlugins:
    - type: Wasm
      enable: true
      config:
        source:
          oci:
            image: localhost:5000/wasm/header-demo:1.0.0
        failOpen: false
        timeoutMs: 500
```

**Digest-pinned form** (always `IfNotPresent`-equivalent — a digest pin
never re-pulls regardless of `pullPolicy`):

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-oci-demo-pinned
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      enable: true
      config:
        source:
          oci:
            image: "localhost:5000/wasm/header-demo@sha256:<manifest-digest-from-step-2>"
        pluginConfig:
          json: {}
        failOpen: false
        timeoutMs: 500
  upstreamResponseFilterPlugins:
    - type: Wasm
      enable: true
      config:
        source:
          oci:
            image: "localhost:5000/wasm/header-demo@sha256:<manifest-digest-from-step-2>"
        failOpen: false
        timeoutMs: 500
```

Wire either one up to a route the same way the integration conf does — reuse
the pattern in
`edgion-tests/integration/conf/EdgionPlugins/Wasm/HTTPRoute_wasm.yaml`
(an `HTTPRoute` with hostname `wasm.example.com`, an `ExtensionRef` filter
pointing at the `EdgionPlugins` name above, forwarding to any HTTP backend)
attached to your test `Gateway`. Apply both documents:

```bash
kubectl apply -f wasm-oci-demo.yaml -f httproute-wasm-oci.yaml
```

## Step 4: Verify the request path

```bash
curl -sD - -H 'Host: wasm.example.com' http://<gateway-address>/ -o /dev/null
```

The response headers must include:

```
x-wasm-resp: yes
```

(added by the `upstreamResponseFilterPlugins` entry). Confirming
`x-wasm-added: yes` needs a backend that echoes what it received: the
request-phase plugin adds that header to the request *before* it reaches the
backend, so it never appears in the client-visible response unless the
backend reflects it back. Two easy options:

- Point the route at `edgion-tests`' own `test_server` binary (the backend
  the local-process integration harness already runs on port 30001) and
  inspect the JSON response body it returns, or its own request log.
- Point the route at a generic echo image (e.g. `ealen/echo-server`, used in
  `docs/en/getting-started/first-gateway.md`) and check the request headers
  section of its JSON response body.

This is the same assertion the automated integration suite makes for
`source.file` in
`edgion-tests/src/client/suites/edgion_plugins/wasm/wasm.rs` — only the
bytecode's origin changed.

## Step 5: Restart-then-disk-hit check

This proves the disk cache is actually load-bearing, not just present.

1. Confirm the gateway's work directory now has a populated
   `wasm-oci-cache/` (or your configured `wasm_oci_cache_dir`):

   ```bash
   ls <gateway-work-dir>/wasm-oci-cache/
   # <hex>.manifest.json  <hex>.blob  ...
   ```

2. **Stop the registry** so any live network pull would visibly fail:

   ```bash
   docker stop edgion-oci-e2e
   ```

3. Restart the gateway process (a fresh process has an empty in-memory
   module cache, so this forces a real `get_or_load` on the next request).

4. Re-run the Step 4 `curl`.

   - **Digest-pinned plugin**: succeeds unchanged — both the manifest and
     the blob are loaded straight from disk by content digest, with zero
     registry traffic (the registry is stopped and it still works).
   - **Tag-form plugin**: fails to load (tag manifests are always
     re-resolved over the network to catch a moved tag) — this is expected;
     restart the registry (`docker start edgion-oci-e2e`) to bring it back,
     then confirm the plugin loads again and that only a manifest `GET`
     hits the registry, never a blob `GET`, by tailing the registry's
     container logs (`docker logs -f edgion-oci-e2e`) while restarting the
     gateway once more.

## Step 6: Private registry (htpasswd + pull secret)

Stand up a second, credential-gated registry:

```bash
mkdir -p auth
# htpasswd comes from the httpd:2 image — registry:2 does not ship the binary.
docker run --rm --entrypoint htpasswd httpd:2 \
  -Bbn testuser testpassword > auth/htpasswd

docker run -d --rm \
  -p 5001:5000 \
  --name edgion-oci-e2e-private \
  -v "$(pwd)/auth:/auth" \
  -e "REGISTRY_AUTH=htpasswd" \
  -e "REGISTRY_AUTH_HTPASSWD_REALM=Registry Realm" \
  -e "REGISTRY_AUTH_HTPASSWD_PATH=/auth/htpasswd" \
  registry:2
```

Push with credentials (the script sends them as an eager `Basic` header,
which `registry:2`'s `htpasswd` auth accepts directly — no bearer-token
exchange involved):

```bash
python3 edgion-tests/wasm/oci/push_wasm_image.py \
  --registry localhost:5001 \
  --repo wasm/header-demo \
  --tag 1.0.0 \
  --wasm edgion-tests/wasm/artifacts/header_demo.wasm \
  --variant oci \
  --username testuser --password testpassword
```

Build the matching `kubernetes.io/dockerconfigjson` Secret. The gateway
reads `resolved_pull_secret` (the controller-resolved raw dockerconfigjson
document) and picks the entry keyed by the registry host, so the `Secret`
must be created against the *registry* host+port used in `source.oci.image`
(`localhost:5001` here):

```bash
kubectl create secret docker-registry wasm-oci-pull-creds \
  --docker-server=localhost:5001 \
  --docker-username=testuser \
  --docker-password=testpassword \
  --docker-email=noreply@example.com \
  --namespace edgion-default \
  --dry-run=client -o yaml
```

which renders:

```yaml
apiVersion: v1
data:
  .dockerconfigjson: eyJhdXRocyI6eyJsb2NhbGhvc3Q6NTAwMSI6eyJ1c2VybmFtZSI6InRlc3R1c2VyIiwicGFzc3dvcmQiOiJ0ZXN0cGFzc3dvcmQiLCJlbWFpbCI6Im5vcmVwbHlAZXhhbXBsZS5jb20iLCJhdXRoIjoiZEdWemRIVnpaWEk2ZEdWemRIQmhjM04zYjNKayJ9fX0=
kind: Secret
metadata:
  creationTimestamp: null
  name: wasm-oci-pull-creds
  namespace: edgion-default
type: kubernetes.io/dockerconfigjson
```

Apply it (drop the `creationTimestamp: null` line, or leave it — the API
server ignores it), then reference it from `source.oci.pullSecretRef`:

```bash
kubectl apply -f - <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: wasm-oci-pull-creds
  namespace: edgion-default
type: kubernetes.io/dockerconfigjson
data:
  .dockerconfigjson: eyJhdXRocyI6eyJsb2NhbGhvc3Q6NTAwMSI6eyJ1c2VybmFtZSI6InRlc3R1c2VyIiwicGFzc3dvcmQiOiJ0ZXN0cGFzc3dvcmQiLCJlbWFpbCI6Im5vcmVwbHlAZXhhbXBsZS5jb20iLCJhdXRoIjoiZEdWemRIVnpaWEk2ZEdWemRIQmhjM04zYjNKayJ9fX0=
EOF
```

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: wasm-oci-private-demo
  namespace: edgion-default
spec:
  requestPlugins:
    - type: Wasm
      enable: true
      config:
        source:
          oci:
            image: localhost:5001/wasm/header-demo:1.0.0
            pullSecretRef:
              name: wasm-oci-pull-creds
        pluginConfig:
          json: {}
        failOpen: false
        timeoutMs: 500
```

Apply and re-run the Step 4 `curl` against a route wired to
`wasm-oci-private-demo`. If the Secret is missing or not yet resolved, the
gateway fails the load rather than falling back to an anonymous pull (see
`resolve_oci_pull_secret` in
`edgion-controller/.../resolve_credential/wasm.rs`); check the controller
logs for `"Wasm oci: pull secret not found yet"` if it doesn't come up
immediately.

## Step 7: `pullPolicy: Always` — live reload

Set an explicit `pullPolicy: Always` on the tag-form plugin from Step 3:

```yaml
        source:
          oci:
            image: localhost:5000/wasm/header-demo:1.0.0
            pullPolicy: Always
```

1. Confirm the Step 4 `curl` works against the current image.
2. Push a **different** wasm file under the **same tag** — for a quick
   negative signal, push an intentionally different guest (or just push the
   `compat`-shaped layer under the `oci` repo/tag to change the manifest
   digest):

   ```bash
   python3 edgion-tests/wasm/oci/push_wasm_image.py \
     --registry localhost:5000 \
     --repo wasm/header-demo \
     --tag 1.0.0 \
     --wasm edgion-tests/wasm/artifacts/header_demo.wasm \
     --variant compat
   ```

   (Pushing the `compat` layer to the `oci` repo's tag changes the
   manifest's layer `mediaType` and digest even though the underlying
   `.wasm` bytes are unchanged, which is enough to trigger a reload; use a
   genuinely different `.wasm` file if you want to observe a behavior
   change through `curl` as well.)
3. Wait up to 60 seconds (`OCI_ALWAYS_RECHECK_SECS`) and re-run the Step 4
   `curl` a few times. The gateway re-checks the tag's manifest digest on
   `get_or_load` at most once per 60s window; once it observes the new
   digest it reloads the module in place. A re-check failure (e.g. the
   registry is briefly unreachable) fails open and keeps serving the
   previously loaded module.

## Cleanup

```bash
docker stop edgion-oci-e2e edgion-oci-e2e-private
rm -rf auth
```
