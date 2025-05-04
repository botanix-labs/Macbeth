#! /bin/env bash

# Install rustc dependencies for reth and btc-server
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    libclang-dev \
    llvm-dev \
    protobuf-compiler \
    gcc \
    pkg-config \
    libprotobuf-dev 


# Download and install bitcoin core
wget https://bitcoin.org/bin/bitcoin-core-28.1/bitcoin-28.1-x86_64-linux-gnu.tar.gz
tar -xzf bitcoin-28.1-x86_64-linux-gnu.tar.gz
sudo mv bitcoin-28.1/bin/bitcoin-cli /usr/local/bin/bitcoin-cli
sudo mv bitcoin-28.1/bin/bitcoin-tx /usr/local/bin/bitcoin-tx
sudo mv bitcoin-28.1/bin/bitcoin-wallet /usr/local/bin/bitcoin-wallet
sudo mv bitcoin-28.1/bin/bitcoind /usr/local/bin/bitcoind


# Download and install cometbft v1.0.1
wget https://github.com/cometbft/cometbft/releases/download/v1.0.1/cometbft_1.0.1_linux_amd64.tar.gz
tar -xzf cometbft_1.0.1_linux_amd64.tar.gz
sudo mv cometbft /usr/local/bin/cometbft
rm cometbft_1.0.1_linux_amd64.tar.gz


