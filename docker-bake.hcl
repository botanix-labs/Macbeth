group "default" {
  targets = ["btc-server", "reth-node"]
}

target "btc-server" {
  dockerfile = "Dockerfile"
  context = "."
  platforms = ["linux/amd64", "linux/arm64"]
  args = {
    BIN = "btc-server"
  }
}

target "reth-node" {
  dockerfile = "Dockerfile"
  context = "."
  platforms = ["linux/amd64", "linux/arm64"]
  args = {
    BIN = "reth"
  }
}
