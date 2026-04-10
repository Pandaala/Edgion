---
layout: home

hero:
  name: Edgion
  text: High-Performance Kubernetes Gateway
  tagline: Built on Pingora and Kubernetes Gateway API — HTTP, gRPC, TCP, UDP, TLS, WebSocket
  actions:
    - theme: brand
      text: Get Started
      link: /en/getting-started/installation-and-deployment
    - theme: alt
      text: 中文文档
      link: /zh-CN/getting-started/installation-and-deployment

features:
  - title: Gateway API v1
    details: Full conformance with Kubernetes Gateway API v1. GatewayClass, Gateway, HTTPRoute, GRPCRoute, TCPRoute, UDPRoute all supported.
  - title: Multi-Protocol
    details: HTTP/1.1, HTTP/2, gRPC, TCP, UDP, WebSocket, TLS termination, mTLS, ACME automatic certificate.
  - title: Rich Plugin System
    details: Built-in plugins covering auth, rate limiting, rewriting, observability, and more. Compose plugins at route or gateway level.
  - title: High Performance
    details: Powered by Cloudflare Pingora. BoringSSL or Rustls backend, jemalloc allocator, async-first architecture.
---
