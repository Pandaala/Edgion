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

ARG RUST_VERSION=1.92

FROM rust:${RUST_VERSION}-bookworm

ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    libclang-dev \
    cmake \
    protobuf-compiler \
    lld \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set LIBCLANG_PATH for bindgen
ENV LIBCLANG_PATH=/usr/lib/llvm-14/lib

# Create project directory
WORKDIR /project

# Default command
CMD ["cargo", "build", "--release"]
