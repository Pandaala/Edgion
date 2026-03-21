# Core Layout

> Final module layout after the bin-oriented refactor.
> Use this file as the placement rule when adding new code under `src/core/`.

## Top Level Rule

`src/core/` now has only four first-level groups:

```text
src/core/
├── controller/   # edgion-controller owned logic
├── gateway/      # edgion-gateway owned logic
├── ctl/          # edgion-ctl owned logic
└── common/       # shared code used across bins
```

Placement rule:

- Put code in `controller/` if it only serves the control plane.
- Put code in `gateway/` if it only serves the data plane.
- Put code in `ctl/` if it only serves the CLI tool.
- Put code in `common/` only when at least two bins depend on it and the code has no hidden runtime coupling.

## Gateway Layout

`src/core/gateway/` is organized by subsystem, not by technical primitive:

```text
src/core/gateway/
├── api/          # Gateway admin/testing APIs
├── backends/     # backend discovery / health / policy
├── cli/          # gateway bootstrap wiring
├── config/       # GatewayClass / EdgionGatewayConfig handlers and stores
├── conf_sync/    # gRPC config client + client cache
├── lb/           # LB policy implementations
├── link_sys/     # external system providers + runtime store
├── observe/      # access_log / metrics / ssl/tcp/udp/sys log
├── plugins/      # http plugins / stream plugins / plugin runtime
├── routes/       # http / grpc / tcp / tls / udp route logic
├── runtime/      # server bootstrap / matching / runtime stores
├── services/     # gateway-side services
└── tls/          # TLS runtime / store / validation
```

Second-level rules inside `gateway/`:

- `runtime/` is only for the Pingora-facing runtime core.
- `routes/` is only for route managers, matchers, and protocol services.
- `plugins/` is only for plugin implementations and plugin execution framework.
- `backends/` is only for upstream discovery, health filtering, and backend policies.
- `tls/` is only for downstream TLS certificate handling and TLS validation.
- `link_sys/` is only for external systems declared by `LinkSys`.
- `config/` is only for config resources that shape gateway runtime, not for gRPC sync machinery.

## Controller Layout

`src/core/controller/` stays focused on the control plane:

```text
src/core/controller/
├── api/          # controller admin API
├── cli/          # controller entry wiring
├── conf_mgr/     # config center, workqueue, resource processor
├── conf_sync/    # gRPC config server + server cache
├── observe/      # controller logging facade
└── services/     # controller-side services
```

Placement rule:

- Anything tied to K8s watch/list, parse/preparse, ref management, or status write-back belongs under `conf_mgr/`.
- Anything tied to gateway-facing config distribution belongs under `conf_sync/`.

## Common Layout

`src/core/common/` is intentionally small:

```text
src/core/common/
├── conf_sync/    # shared proto / traits / sync types
├── config/       # shared startup config
├── matcher/      # shared host/ip/radix matchers
└── utils/        # reusable utility code
```

Do not move code into `common/` just to avoid imports. Keep business ownership explicit unless the code is genuinely shared.

## Anti-Rules

Avoid reintroducing these patterns:

- top-level `src/core/api`, `src/core/cli`, `src/core/conf_sync`, `src/core/services`
- flat gateway buckets like `gateway/http_routes`, `gateway/edgion_plugins`, `gateway/health_check`
- new compatibility shims that hide real ownership

If a new subsystem needs a home, prefer creating a clear owned directory under the correct bin group instead of adding another cross-cutting top-level bucket.
