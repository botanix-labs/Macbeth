# syntax = docker/dockerfile:1.7-labs

FROM rust:1.85 AS base

# TODO: Make base image or combine into one multistage image for DRY

ARG TARGETARCH
ARG TARGETOS

# Download and install cargo-binstall
ENV BINSTALL_VERSION=1.12.6
RUN set -ex; \
    if [ "$TARGETARCH" = "amd64" ]; then \
        CARGO_BINSTALL_ARCH="x86_64-unknown-linux-musl"; \
    elif [ "$TARGETARCH" = "arm64" ]; then \
        CARGO_BINSTALL_ARCH="aarch64-unknown-linux-musl"; \
    else \
        echo "Unsupported architecture: $TARGETARCH"; exit 1; \
    fi; \
    # Construct download URL
    DOWNLOAD_URL="https://github.com/cargo-bins/cargo-binstall/releases/download/v${BINSTALL_VERSION}/cargo-binstall-${CARGO_BINSTALL_ARCH}.tgz"; \
    # Download and extract the cargo-binstall binary
    curl -A "Mozilla/5.0 (X11; Linux x86_64; rv:60.0) Gecko/20100101 Firefox/81.0" -L --proto '=https' --tlsv1.2 -sSf "$DOWNLOAD_URL" | tar -xvzf -;  \
    ./cargo-binstall -y --force cargo-binstall@${BINSTALL_VERSION}; \
    rm ./cargo-binstall; \
    cargo binstall -V

RUN cargo binstall --locked cargo-chef sccache

RUN apt-get update && apt-get install -y \
    libclang-dev \
    pkg-config \
    build-essential \
    libssl-dev \
    protobuf-compiler \
    libprotobuf-dev

ENV RUSTC_WRAPPER=sccache SCCACHE_DIR=/sccache

# Builds a cargo-chef plan
FROM base AS planner

WORKDIR /app

COPY --parents bin crates testing Cargo.toml Cargo.lock .cargo ./

RUN cargo chef prepare --recipe-path recipe.json

FROM base AS builder

WORKDIR /app

COPY --from=planner /app/recipe.json recipe.json

# Build profile, release by default
ARG BUILD_PROFILE=release
ENV BUILD_PROFILE=$BUILD_PROFILE

# Extra Cargo flags
ARG RUSTFLAGS=""
ENV RUSTFLAGS="$RUSTFLAGS"

# Extra Cargo features
ARG FEATURES=""
ENV FEATURES=$FEATURES

# Builds dependencies
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook \
    --profile $BUILD_PROFILE \
    --features "$FEATURES" \
    --recipe-path recipe.json \
    --bin reth \
    --locked

# Build application
COPY --parents bin crates testing Cargo.toml Cargo.lock .cargo ./

RUN cargo build \
    --profile $BUILD_PROFILE \
    --features "$FEATURES" \
    --bin reth \
    --locked

# ARG is not resolved in COPY so we have to hack around it by copying the
# binary to a temporary location
RUN if [[ "${BUILD_PROFILE}" == "release" ]] ; then \
      OUT_DIRECTORY=release; \
    else \
      OUT_DIRECTORY=debug; \
    fi && \
    cp /app/target/$OUT_DIRECTORY/reth /app/reth

# Use Ubuntu as the release image
FROM ubuntu AS runtime

WORKDIR /app

# Copy reth over from the build stage
COPY --from=builder /app/reth /usr/local/bin

# Copy licenses
COPY LICENSE-* ./

EXPOSE 30303 30303/udp 9001 8545 8546
ENTRYPOINT ["/usr/local/bin/reth"]
