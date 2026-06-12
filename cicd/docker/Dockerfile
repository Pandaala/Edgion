# Edgion Multi-stage Dockerfile
# Supports building: edgion-gateway, edgion-controller, test_server, test_client
# Each image includes edgion-cli for debugging (bin only)
#
# Build arguments:
#   BINARY: Target binary name (default: edgion-gateway)
#   BUILD_TYPE: Build type - bin or example (default: bin)
#   FEATURES: Cargo features (default: default, which includes boringssl)
#   RUST_VERSION: Rust version (default: 1.92)
#
# Usage:
#   # Build bin targets
#   docker build --build-arg BINARY=edgion-gateway -t edgion/edgion-gateway:0.1.0 -f docker/Dockerfile .
#   docker build --build-arg BINARY=edgion-controller -t edgion/edgion-controller:0.1.0 -f docker/Dockerfile .
#
#   # Build example targets
#   docker build --build-arg BUILD_TYPE=example --build-arg BINARY=test_server -t edgion/edgion-test-server:0.1.0 -f docker/Dockerfile .
#   docker build --build-arg BUILD_TYPE=example --build-arg BINARY=test_client -t edgion/edgion-test-client:0.1.0 -f docker/Dockerfile .

# Build arguments
ARG RUST_VERSION=1.92
ARG BINARY=edgion-gateway
ARG BUILD_TYPE=bin
ARG FEATURES=default
ARG PROFILE=release

# =============================================================================
# Stage 1: Chef Planner - Analyze dependencies
# =============================================================================
FROM rust:${RUST_VERSION}-slim AS chef

WORKDIR /app

# Install cargo-chef for dependency caching
RUN cargo install cargo-chef

# =============================================================================
# Stage 2: Chef Cook - Cache dependencies
# =============================================================================
FROM chef AS planner

# Virtual workspace (no root package since the crate split): cargo-chef needs the
# full member manifest graph to compute the recipe. Copying the whole tree is the
# documented workspace pattern — `cargo chef prepare` only reads the manifests and
# Cargo.lock, so `recipe.json` stays stable across source-only changes and the
# expensive `cook` layer below keeps its cache.
COPY . .

# Generate dependency recipe
RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# Stage 3: Builder - Build dependencies and application
# =============================================================================
FROM rust:${RUST_VERSION}-slim AS builder

WORKDIR /app

# Install additional build dependencies
# lld: faster linker, lower peak memory than ld (important for debug builds)
RUN apt-get update && apt-get install -y \
    g++ \
    cmake \
    git \
    libclang-dev \
    lld \
    perl \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Use lld as the linker for all targets to reduce link-time memory usage
ENV RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    CARGO_BUILD_JOBS=4

# Install cargo-chef
RUN cargo install cargo-chef

# Copy dependency recipe from planner
COPY --from=planner /app/recipe.json recipe.json

# Build arguments
ARG FEATURES
ARG BINARY
ARG BUILD_TYPE
ARG PROFILE

# Build dependencies (cached layer).
# In a virtual workspace `--features` is only valid alongside a package selection
# (`-p`), so cook the feature-bearing crate the final build below produces. Only
# the product crates (gateway/controller) carry the boringssl/rustls feature —
# edgion-cli has no such feature, so it is NOT cooked with `--features` here; its
# few unique deps fall through to the final build (the heavy pingora/boringssl
# deps belong to the product crate and are already cooked).
#   - example: the edgion-tests harness crate (defines no TLS feature).
#   - bin:     the product crate carrying the TLS backend feature.
RUN PROFILE_FLAG=$([ "${PROFILE}" = "release" ] && echo "--release" || echo ""); \
    if [ "$BUILD_TYPE" = "example" ]; then \
      cargo chef cook ${PROFILE_FLAG} -p edgion-tests --recipe-path recipe.json; \
    else \
      cargo chef cook ${PROFILE_FLAG} -p "${BINARY}" --features "${FEATURES}" --recipe-path recipe.json; \
    fi

# Copy source code
COPY . .

# Ensure config directory exists (for optional config files)
RUN mkdir -p /app/config && touch /app/config/.placeholder

# Build based on BUILD_TYPE and PROFILE
RUN PROFILE_FLAG=$([ "${PROFILE}" = "release" ] && echo "--release" || echo ""); \
    OUT_DIR=$([ "${PROFILE}" = "release" ] && echo "release" || echo "debug"); \
    if [ "$BUILD_TYPE" = "example" ]; then \
      echo "Building edgion-tests bin: ${BINARY} (${PROFILE})"; \
      cargo build ${PROFILE_FLAG} -p edgion-tests --bin "${BINARY}"; \
      cp /app/target/${OUT_DIR}/${BINARY} /app/edgion-binary; \
      touch /app/edgion-cli-binary; \
    else \
      echo "Building bin: ${BINARY} (${PROFILE})"; \
      cargo build ${PROFILE_FLAG} -p "${BINARY}" --features "${FEATURES}" --bin "${BINARY}"; \
      cargo build ${PROFILE_FLAG} -p edgion-cli --bin edgion-cli; \
      cp /app/target/${OUT_DIR}/${BINARY} /app/edgion-binary; \
      cp /app/target/${OUT_DIR}/edgion-cli /app/edgion-cli-binary; \
    fi

# =============================================================================
# Stage 4: Runtime - Minimal runtime environment
# =============================================================================
FROM debian:trixie-slim AS runtime

# Build arguments
ARG BINARY
ARG BUILD_TYPE
ARG PROFILE

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -g 1000 edgion && \
    useradd -r -u 1000 -g edgion edgion

WORKDIR /usr/local/edgion
RUN chown -R edgion:edgion /usr/local/edgion

# Copy binaries from builder
# Primary location: /usr/local/edgion/${BINARY} — owned by edgion, supports live hot-patch via kubectl cp.
# Symlink from /usr/local/bin/ for compatibility with tools that expect it there.
COPY --from=builder --chown=edgion:edgion /app/edgion-binary /usr/local/edgion/${BINARY}
RUN chmod +x /usr/local/edgion/${BINARY} && \
    ln -sf /usr/local/edgion/${BINARY} /usr/local/bin/${BINARY}

# Copy cli for bin builds only (example builds have empty file)
COPY --from=builder /app/edgion-cli-binary /tmp/edgion-cli-binary
RUN if [ "$BUILD_TYPE" != "example" ]; then \
      mv /tmp/edgion-cli-binary /usr/local/bin/edgion-cli && \
      chmod +x /usr/local/bin/edgion-cli; \
    else \
      rm -f /tmp/edgion-cli-binary; \
    fi

# Copy default config files from builder (if any exist)
COPY --from=builder --chown=edgion:edgion /app/config ./config

# Switch to non-root user
USER edgion

# Environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=full

# Expose ports
EXPOSE 80 443 10080 10443 18443 19000 19002 19010 12151 12101

# Restart-loop entrypoint: when the binary is killed (e.g. by kubectl exec pkill during hot-patch),
# the shell restarts it automatically without triggering a K8s pod restart.
# This enables: kubectl cp new-binary → pkill → binary restarts in-place.
ENTRYPOINT ["/bin/sh", "-c"]
CMD ["while true; do /usr/local/edgion/${BINARY}; echo '[edgion] process exited, restarting in 1s...'; sleep 1; done"]

# Labels
LABEL maintainer="Edgion Team"
LABEL org.opencontainers.image.source="https://github.com/Pandaala/Edgion"
LABEL org.opencontainers.image.description="Edgion Gateway - Kubernetes Gateway API Implementation"
LABEL org.opencontainers.image.licenses="Apache-2.0"
