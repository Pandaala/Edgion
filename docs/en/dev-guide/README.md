# Edgion Developer Guide

This directory is intended for Edgion contributors, focusing on architecture design, resource processing pipelines, plugin extensions, and internal observability implementation.

## Architecture Main Line

### 1. Overall System Architecture

- [Architecture Overview](./architecture-overview.md): Control plane/data plane, module boundaries, request path overview.
- [Resource Architecture Overview](./resource-architecture-overview.md): Resource synchronization and processing pipeline (watch/list, caching, parsing, distribution).

### 2. Resource Processing and Registration

- [Resource Registry Guide](./resource-registry-guide.md): How resource types integrate into the unified registry system.
- [Adding New Resource Types Guide](./add-new-resource-guide.md): Complete steps for adding a new CRD.

### 3. Configuration Extension Mechanisms

- [Annotations Guide](./annotations-guide.md): `edgion.io/*` annotation design, parsing, and runtime behavior.
- [HTTP Plugin Development Guide](./http-plugin-development.md): `EdgionPlugins` execution stages, runtime wiring, and implementation boundaries.
- [Stream Plugin Development Guide](./stream-plugin-development.md): the `EdgionStreamPlugins` two-stage model, implementation boundaries, and wiring points.

### 4. Gateway Infrastructure

- [Work Directory Design](./work-directory.md): Work directory resolution, priority, and migration strategy.
- [Logging System Architecture](./logging-system.md): Access/SSL/TCP/UDP log pipeline and output system.

### 5. Build and Release

- [CI and Release Workflow Guide](./ci-release-workflow.md): GitHub Actions, the shared `setup-rust` action, local reproduction, and image publishing flow.

### 6. Design Review Documents

- [JWT Auth Plugin Design](./jwt-auth-plugin-design.md): Plugin design phase document example (feature and configuration review).

### 7. AI Collaboration

- [AI Collaboration and Skill Usage Guide](./ai-agent-collaboration.md): How to let AI tools navigate `AGENTS.md`, `skills/`, and `docs/` without manually pasting long document lists.

### 8. Knowledge Source Map

- [Knowledge Source Map and Maintenance Rules](./knowledge-source-map.md): Defines the responsibilities of `AGENTS.md`, `skills/`, `docs/`, and thin platform wrappers so they do not drift.

## Recommended Reading Order

1. [Architecture Overview](./architecture-overview.md)
2. [Resource Architecture Overview](./resource-architecture-overview.md)
3. [Resource Registry Guide](./resource-registry-guide.md)
4. [Adding New Resource Types Guide](./add-new-resource-guide.md)
5. [Annotations Guide](./annotations-guide.md)
6. [HTTP Plugin Development Guide](./http-plugin-development.md)
7. [Stream Plugin Development Guide](./stream-plugin-development.md)
8. [Logging System Architecture](./logging-system.md)
9. [CI and Release Workflow Guide](./ci-release-workflow.md)
10. [AI Collaboration and Skill Usage Guide](./ai-agent-collaboration.md)
11. [Knowledge Source Map and Maintenance Rules](./knowledge-source-map.md)

## Directory Positioning Principles

- `dev-guide`: Source code internals, architecture design, contribution workflow.
- `ops-guide`: Gateway/GatewayClass, listeners, TLS, observability, infrastructure operations.
- `user-guide`: HTTPRoute/TCPRoute/GRPCRoute/UDPRoute configuration and plugin usage.

If a topic involves multiple reader types, write separate documents and cross-reference them, rather than mixing them in a single document.

## Documentation Maintenance Best Practices

1. Update the corresponding directory `README.md` whenever a document is added or removed.
2. Only link to existing documents; do not add placeholder "(coming soon)" links. Track planned topics in the knowledge-source map or team backlog instead.
3. For capabilities that are not part of the standard Gateway API, clearly mark them as Edgion extensions at the beginning of the document.
4. Implicit logic that affects request behavior (defaults, execution order, auto-completion) must be explicitly documented.
5. Write user documentation and developer documentation separately: one covers "how to configure", the other covers "how it's implemented".
6. After changing `AGENTS.md`, `skills/`, or these dev-guide entry docs, run `make check-agent-docs`.
