# Edgion Build Environment
# Used for compiling Rust binaries in Docker (supports multi-arch)
#
# This Dockerfile creates a build environment that can compile
# Linux binaries for any architecture (arm64/amd64) from any host.
#
# Usage:
#   # Build the image for arm64
#   docker build --platform linux/arm64 -t edgion-builder -f docker/Dockerfile.builder .
#
#   # Run compilation
#   docker run --rm --platform linux/arm64 \
#     -v $(pwd):/project \
#     -v ~/.cargo/registry:/usr/local/cargo/registry \
#     edgion-builder \
#     cargo build --release --target aarch64-unknown-linux-gnu

ARG RUST_VERSION=1.96.1
ARG GO_VERSION=1.25.0

FROM golang:${GO_VERSION}-bookworm AS go-toolchain

FROM rust:${RUST_VERSION}-bookworm

COPY --from=go-toolchain /usr/local/go /usr/local/go

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH="/usr/local/go/bin:${PATH}"

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    cmake \
    curl \
    libclang-dev \
    libssl-dev \
    lld \
    lsof \
    netcat-openbsd \
    openssl \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Set LIBCLANG_PATH for bindgen
ENV LIBCLANG_PATH=/usr/lib/llvm-14/lib

# Create project directory
WORKDIR /project

# Default command
CMD ["cargo", "build", "--release"]
