group "default" {
  targets = ["btc-server", "reth-node"]
}

target "btc-server" {
  dockerfile = "Dockerfile"
  context = "."
  platforms = ["linux/amd64", "linux/arm64"]
  target = "btc-server"
}

target "reth-node" {
  dockerfile = "Dockerfile"
  context = "."
  platforms = ["linux/amd64", "linux/arm64"]
  target = "reth"
}
