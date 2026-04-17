# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **GlobalConnectionIpRestriction** — new `PluginMetaData::GlobalConnectionIpRestriction` variant and
  `EdgionStreamPlugin::GlobalConnectionIpRestriction` variant for L4 TCP connection-phase IP restriction.
  Centrally managed by Center with per-Controller fan-out CRUD, PATCH shortcuts, and progressive sync.

  New Center REST endpoints (all under `/api/v1/center/global-connection-ip-restrictions`):
  - `GET  /` — aggregated list across all Controllers
  - `GET  /consistency` — content-hash-based consistency check
  - `GET  /{ns}/{name}` — single resource detail view
  - `POST /` — create on specified controllers (fan-out)
  - `PUT  /{ns}/{name}` — full update (fan-out)
  - `DELETE /{ns}/{name}` — delete (fan-out)
  - `PATCH /{ns}/{name}/enable` — fast toggle for `enable` flag
  - `PATCH /{ns}/{name}/active-profile` — fast switch between named profiles
  - `POST /{ns}/{name}/sync` — copy resource from source → target controllers

  New frontend page: **"Global IP Restrictions"** in the Center sidebar.

### Changed
- 

### Fixed
- 

### Removed
- 

### Breaking Changes

- **`EdgionStreamPlugin::IpRestriction`** — now uses `ConnectionIpRestrictionRules` (L4-only) instead of
  the HTTP `IpRestrictionConfig`. The following fields have been **removed** and will cause deserialization
  errors if present in existing YAML (`serde deny_unknown_fields` enforced):

  | Removed field | Reason |
  |---|---|
  | `ipSource` | L4 has no headers; only `RemoteAddr` is available |
  | `message`  | L4 rejection is a silent TCP close; no protocol-level response message |
  | `status`   | L4 has no HTTP status codes |

  **Migration** — remove all three fields from every `type: IpRestriction` entry under
  `EdgionStreamPlugins.spec.plugins[]`:

  ```yaml
  # Before (fails to parse — unknown fields)
  - type: IpRestriction
    config:
      allow: ["192.168.1.0/24"]
      ipSource: RemoteAddr      # DELETE
      message: "blocked"        # DELETE
      status: 403               # DELETE
      defaultAction: deny

  # After
  - type: IpRestriction
    config:
      allow: ["192.168.1.0/24"]
      defaultAction: deny
  ```

  > **Note:** `TlsRouteStreamPlugin::IpRestriction` (Stage 2, `spec.tlsRoutePlugins`) still uses
  > the HTTP `IpRestrictionConfig` (post-TLS handshake with SNI context) and is **unchanged**.

---

## [0.1.0] - 2026-01-06

### Added
- Initial release of Edgion Gateway
- Full Kubernetes Gateway API v1 support
- High-performance data plane built on Pingora
- HTTP/1.1, HTTP/2, gRPC, TCP, UDP, WebSocket protocol support
- Extensible plugin system
  - HTTP filters (CORS, CSRF, Basic Auth, IP Restriction)
  - TCP/UDP stream plugins
- Advanced TLS support
  - SNI-based dynamic certificate loading
  - mTLS (mutual TLS)
  - Backend TLS with CA verification
- Load balancing policies
  - Round Robin
  - Random
  - Consistent Hash (header, cookie, IP)
- Observability features
  - Structured JSON access logs
  - Request/response timing metrics
- edgion-controller: Configuration management component
- edgion-gateway: High-performance proxy component
- edgion-cli: CLI tool for management and debugging
- Multi-language documentation (Chinese, English, Japanese)

### Infrastructure
- GitHub Actions CI/CD pipeline
- Docker image builds for gateway and controller
- Integration test framework

---

<!-- Links -->
[Unreleased]: https://github.com/Pandaala/Edgion/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Pandaala/Edgion/releases/tag/v0.1.0

