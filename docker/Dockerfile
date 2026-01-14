# Edgion Multi-stage Dockerfile
# Supports building: edgion-gateway, edgion-controller, test_server, test_client
# Each image includes edgion-ctl for debugging (bin only)
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

# Copy only Cargo files to analyze dependencies
COPY Cargo.toml Cargo.lock ./
COPY src/lib.rs src/lib.rs
COPY src/bin src/bin

# Generate dependency recipe
RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# Stage 3: Builder - Build dependencies and application
# =============================================================================
FROM rust:${RUST_VERSION} AS builder

WORKDIR /app

# Install additional build dependencies
RUN apt-get update && apt-get install -y \
    cmake \
    libclang-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-chef
RUN cargo install cargo-chef

# Copy dependency recipe from planner
COPY --from=planner /app/recipe.json recipe.json

# Build arguments
ARG FEATURES
ARG BINARY
ARG BUILD_TYPE

# Build dependencies (cached layer)
RUN cargo chef cook --release --features "${FEATURES}" --recipe-path recipe.json

# Copy source code
COPY . .

# Build based on BUILD_TYPE
# For bin: build binary and ctl
# For example: build example only (no ctl bundled)
RUN if [ "$BUILD_TYPE" = "example" ]; then \
      echo "Building example: ${BINARY}"; \
      cargo build --release --features "${FEATURES}" --example "${BINARY}"; \
      cp /app/target/release/examples/${BINARY} /app/edgion-binary; \
      touch /app/edgion-ctl-binary; \
    else \
      echo "Building bin: ${BINARY}"; \
      cargo build --release --features "${FEATURES}" --bin "${BINARY}" --bin edgion-ctl; \
      cp /app/target/release/${BINARY} /app/edgion-binary; \
      cp /app/target/release/edgion-ctl /app/edgion-ctl-binary; \
    fi

# =============================================================================
# Stage 4: Runtime - Minimal runtime environment
# =============================================================================
FROM debian:bookworm-slim AS runtime

# Build arguments
ARG BINARY
ARG BUILD_TYPE

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -g 1000 edgion && \
    useradd -r -u 1000 -g edgion edgion

# Create necessary directories
RUN mkdir -p /usr/local/edgion/{config,logs,runtime} && \
    chown -R edgion:edgion /usr/local/edgion

# Set working directory
WORKDIR /usr/local/edgion

# Copy binaries from builder
COPY --from=builder /app/edgion-binary /usr/local/bin/${BINARY}
RUN chmod +x /usr/local/bin/${BINARY}

# Copy ctl for bin builds only (example builds have empty file)
COPY --from=builder /app/edgion-ctl-binary /tmp/edgion-ctl-binary
RUN if [ "$BUILD_TYPE" != "example" ]; then \
      mv /tmp/edgion-ctl-binary /usr/local/bin/edgion-ctl && \
      chmod +x /usr/local/bin/edgion-ctl; \
    else \
      rm -f /tmp/edgion-ctl-binary; \
    fi

# Copy default config files (optional, may not exist for examples)
COPY --chown=edgion:edgion config/*.toml ./config/ 2>/dev/null || true

# Switch to non-root user
USER edgion

# Environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=0

# Expose ports
EXPOSE 80 443 10080 10443 18443 19000 19002 19010 50051 5800

# Set entrypoint
ENTRYPOINT ["/bin/sh", "-c"]
CMD ["exec /usr/local/bin/${BINARY}"]

# Labels
LABEL maintainer="Edgion Team"
LABEL org.opencontainers.image.source="https://github.com/Pandaala/Edgion"
LABEL org.opencontainers.image.description="Edgion Gateway - Kubernetes Gateway API Implementation"
LABEL org.opencontainers.image.licenses="Apache-2.0"
