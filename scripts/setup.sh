#!/bin/bash

# Install pre-commit hooks
pre-commit install

# Install nightly toolchain
rustup toolchain install nightly -c rustfmt

# Install cargo global crates
cargo install --locked samply
cargo install cargo-binstall
cargo +stable install cargo-llvm-cov --locked
cargo install cargo-audit --locked --features=fix
cargo install cargo-nextest --locked --features=fix
cargo binstall --no-confirm cargo-watch knope cargo-sort typos-cli
