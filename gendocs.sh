#!/usr/bin/env sh

RUSTDOCSDIR=${PWD}/rustdocs
mkdir -p ${RUSTDOCSDIR}

cargo doc --target-dir ${RUSTDOCSDIR} --all --bins --examples --document-private-items

# This is opinionated, but doesn't matter. Any page has full search.
DEFAULT_CRATE=reth_authority_consensus
echo "Open Rust docs at file://${RUSTDOCSDIR}/doc/${DEFAULT_CRATE}/index.html"
