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

ARG INCLUDE_BORINGSSL_FIPS_SOURCE=false
ARG BORINGSSL_FIPS_BASE_REV=e2a57cfb4d915b4ba820585aef9fdee7bca13fe5
ARG BORINGSSL_FIPS_ARM64_FIX_REV=4d6b2f971e456962b70e4e91768caffdb36fe016
ARG BORINGSSL_FIPS_ARM64_FIX_SHA256=f590a84b210b9e5c32d5d24a4d3f76e618875ee6f3a2bcc31e0a54371670f81f

COPY --from=go-toolchain /usr/local/go /usr/local/go

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH="/usr/local/go/bin:${PATH}"

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    cmake \
    curl \
    git \
    libclang-dev \
    libssl-dev \
    lld \
    lsof \
    netcat-openbsd \
    openssl \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Boring 5.2.0 vendors BORINGSSL_FIPS_BASE_REV. Its AArch64 delocate pass does
# not treat BORINGSSL_bcm_text_hash as synthesized, so the final GOT entry can
# point at the module start and make the runtime integrity self-test abort.
# Keep this source override FIPS-only and verify the exact upstream patch bytes.
# TODO(boring-fips-arm64): Remove this override and update Boring once boring-sys
# vendors BORINGSSL_FIPS_ARM64_FIX_REV or a descendant containing that fix. See
# tasks/todo/boring-upstream-arm64-fips-source-override-removal.md.
RUN if [ "${INCLUDE_BORINGSSL_FIPS_SOURCE}" = "true" ]; then \
        boringssl_fips_source_path="/opt/boringssl-fips-${BORINGSSL_FIPS_BASE_REV}-${BORINGSSL_FIPS_ARM64_FIX_REV}-${BORINGSSL_FIPS_ARM64_FIX_SHA256}" && \
        mkdir -p "${boringssl_fips_source_path}" /tmp/boringssl-fips-fix && \
        git -C "${boringssl_fips_source_path}" init && \
        git -C "${boringssl_fips_source_path}" remote add origin \
            https://boringssl.googlesource.com/boringssl && \
        git -C "${boringssl_fips_source_path}" fetch --depth=1 origin \
            "${BORINGSSL_FIPS_BASE_REV}" && \
        git -C "${boringssl_fips_source_path}" checkout --detach \
            "${BORINGSSL_FIPS_BASE_REV}" && \
        test "$(git -C "${boringssl_fips_source_path}" rev-parse HEAD)" = \
            "${BORINGSSL_FIPS_BASE_REV}" && \
        curl --retry 3 --retry-all-errors -fsSL \
            "https://boringssl.googlesource.com/boringssl/+/${BORINGSSL_FIPS_ARM64_FIX_REV}%5E%21/?format=TEXT" \
            | base64 -d > /tmp/boringssl-fips-fix/upstream.patch && \
        echo "${BORINGSSL_FIPS_ARM64_FIX_SHA256}  /tmp/boringssl-fips-fix/upstream.patch" \
            | sha256sum -c - && \
        git -C "${boringssl_fips_source_path}" apply --check \
            /tmp/boringssl-fips-fix/upstream.patch && \
        git -C "${boringssl_fips_source_path}" apply \
            /tmp/boringssl-fips-fix/upstream.patch && \
        cd "${boringssl_fips_source_path}" && \
        go test ./util/fipstools/delocate && \
        rm -rf /tmp/boringssl-fips-fix; \
    fi

# Set LIBCLANG_PATH for bindgen
ENV LIBCLANG_PATH=/usr/lib/llvm-14/lib

# Create project directory
WORKDIR /project

# Default command
CMD ["cargo", "build", "--release"]
