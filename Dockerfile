# Edgion Multi-stage Dockerfile
# Supports building: edgion-gateway, edgion-controller, edgion-ctl
#
# Build arguments:
#   BINARY: Target binary name (default: edgion-gateway)
#   FEATURES: Cargo features (default: default, which includes boringssl)
#   RUST_VERSION: Rust version (default: 1.92)
#
# Usage:
#   docker build --build-arg BINARY=edgion-gateway -t edgion/edgion-gateway:0.1.0 .
#   docker build --build-arg BINARY=edgion-controller -t edgion/edgion-controller:0.1.0 .
#   docker build --build-arg BINARY=edgion-ctl -t edgion/edgion-ctl:0.1.0 .

# Build arguments
ARG RUST_VERSION=1.92
ARG BINARY=edgion-gateway
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

# Install additional build dependencies (base image already has gcc, make, perl, etc.)
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

# Build dependencies (cached layer)
RUN cargo chef cook --release --features "${FEATURES}" --bin "${BINARY}" --recipe-path recipe.json

# Copy source code
COPY . .

# Build the application
RUN cargo build --release --features "${FEATURES}" --bin "${BINARY}"

# Copy binary to a known location
RUN cp /app/target/release/${BINARY} /app/edgion-binary

# =============================================================================
# Stage 4: Runtime - Minimal runtime environment
# =============================================================================
FROM debian:bookworm-slim AS runtime

# Build arguments
ARG BINARY

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

# Copy binary from builder
COPY --from=builder /app/edgion-binary /usr/local/bin/${BINARY}
RUN chmod +x /usr/local/bin/${BINARY}

# Copy default config files
COPY --chown=edgion:edgion config/*.toml ./config/

# Switch to non-root user
USER edgion

# Environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=0

# Expose ports (will be overridden based on binary)
# Gateway: 80, 443, 10080, 10443
# Controller: 50051, 5800
# CLI: No ports
EXPOSE 80 443 10080 10443 18443 19000 19002 19010 50051 5800

# Set entrypoint
ENTRYPOINT ["/bin/sh", "-c"]
CMD ["exec /usr/local/bin/${BINARY}"]

# Labels
LABEL maintainer="Edgion Team"
LABEL org.opencontainers.image.source="https://github.com/your-org/Edgion"
LABEL org.opencontainers.image.description="Edgion Gateway - Kubernetes Gateway API Implementation"
LABEL org.opencontainers.image.licenses="Apache-2.0"
