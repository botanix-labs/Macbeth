#syntax=docker/dockerfile:1.7-labs

# Build deps
FROM rust:1.85 AS base

ARG TARGETARCH

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

COPY --parents .cargo bin crates testing Cargo.toml Cargo.lock  ./

RUN cargo chef prepare --recipe-path recipe.json

# Builds an application
FROM base AS builder

WORKDIR /app

COPY --from=planner /app/recipe.json recipe.json

# Build profile, release by default
ARG PROFILE=release

# rustc flags
ARG RUSTFLAGS=""
ENV RUSTFLAGS="$RUSTFLAGS"

# Features to enable
ARG FEATURES=""

# Package to build (reth or btc-server)
ARG BIN=reth

# Builds dependencies
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook \
    --profile $PROFILE \
    --features "$FEATURES" \
    --recipe-path recipe.json \
    --package "$BIN" \
    --bin "$BIN" \
    --locked

COPY --parents .cargo bin crates testing Cargo.toml Cargo.lock  ./

# Build application
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    if [ "${PROFILE}" = "release" ] ; then \
      OUT_DIRECTORY=release; \
    else \
      OUT_DIRECTORY=debug; \
    fi && \
    cargo build \
    --profile $PROFILE \
    --features "$FEATURES" \
    --package "$BIN" \
    --bin "$BIN" \
    --locked && \
    cp target/$OUT_DIRECTORY/$BIN /usr/local/bin/app

# Use Ubuntu as the release image
FROM ubuntu AS runtime

WORKDIR /app

# Copy reth over from the build stage
COPY --from=builder /usr/local/bin/app /usr/local/bin/

# Copy licenses
COPY LICENSE-* ./

EXPOSE 30303 30303/udp 30304 9001 8545 8546 8080 7000
ENTRYPOINT ["/usr/local/bin/app"]
