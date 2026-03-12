# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- 

### Changed
- 

### Fixed
- 

### Removed
- 

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
- edgion-ctl: CLI tool for management and debugging
- Multi-language documentation (Chinese, English, Japanese)

### Infrastructure
- GitHub Actions CI/CD pipeline
- Docker image builds for gateway and controller
- Integration test framework

---

<!-- Links -->
[Unreleased]: https://github.com/Pandaala/Edgion/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Pandaala/Edgion/releases/tag/v0.1.0

