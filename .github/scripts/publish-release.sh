#!/bin/bash
set -euo pipefail

# Publish release to public repository
# Usage: publish-release.sh <command> <version> <channel> <release_notes> [git_tag] [git_sha] [prev_version]

COMMAND="$1"
VERSION="$2"
CHANNEL="${3:-stable}"
RELEASE_NOTES="$4"
GIT_TAG="${5:-}"
GIT_SHA="${6:-}"
PREV_VERSION="${7:-}"

# Map channel names
case "$CHANNEL" in
    "latest") CHANNEL="stable" ;;
esac

generate_release_manifest() {
    echo "Generating release manifest for v$VERSION ($CHANNEL)..."
    
    mkdir -p "botanix/releases/$VERSION"
    mkdir -p "botanix/releases/$VERSION/binaries"
    mkdir -p "botanix/releases/$VERSION/docker"
    mkdir -p "botanix/changelog/$CHANNEL"
    
    cat > "botanix/releases/$VERSION/release.json" << EOF
{
  "version": "$VERSION",
  "channel": "$CHANNEL", 
  "release_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "git_tag": "$GIT_TAG",
  "git_sha": "$GIT_SHA",
  "previous_version": "$PREV_VERSION",
  "breaking_changes": $(echo "$RELEASE_NOTES" | grep -i "BREAKING" > /dev/null && echo "true" || echo "false"),
  "prerelease": $([ "$CHANNEL" != "stable" ] && echo "true" || echo "false"),
  "binaries": {
    "reth": {
      "linux_x86_64": {
        "url": "https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz",
        "checksum_url": "https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz.sha256sum"
      },
      "linux_arm64": {
        "url": "https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-aarch64-unknown-linux-gnu.tar.gz", 
        "checksum_url": "https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-aarch64-unknown-linux-gnu.tar.gz.sha256sum"
      }
    },
    "btc-server": {
      "linux_x86_64": {
        "url": "https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-x86_64-unknown-linux-gnu.tar.gz",
        "checksum_url": "https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-x86_64-unknown-linux-gnu.tar.gz.sha256sum"
      },
      "linux_arm64": {
        "url": "https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-aarch64-unknown-linux-gnu.tar.gz",
        "checksum_url": "https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-aarch64-unknown-linux-gnu.tar.gz.sha256sum"
      }
    }
  },
  "docker_images": {
    "btc_server": {
      "registry": "ghcr.io/botanix-labs/botanix-btc-server",
      "tags": ["$VERSION", "$CHANNEL"]
    },
    "reth_node": {
      "registry": "ghcr.io/botanix-labs/botanix-reth-node", 
      "tags": ["$VERSION", "$CHANNEL"]
    }
  }
}
EOF
    
    echo "✅ Generated release manifest"
}

generate_release_readme() {
    echo "Generating release README for v$VERSION..."
    
    cat > "botanix/releases/$VERSION/README.md" << EOF
# Botanix Release v$VERSION

**Release Channel:** \`$CHANNEL\`  
**Release Date:** $(date -u +"%Y-%m-%d %H:%M:%S UTC")  
**Git Tag:** \`$GIT_TAG\`  
**Git SHA:** \`$GIT_SHA\`

## Release Notes

$RELEASE_NOTES

## Downloads

### Binaries

#### Reth Node
- [Linux x86_64](https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz) ([checksum](https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz.sha256sum))
- [Linux ARM64](https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-aarch64-unknown-linux-gnu.tar.gz) ([checksum](https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-aarch64-unknown-linux-gnu.tar.gz.sha256sum))

#### BTC Server  
- [Linux x86_64](https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-x86_64-unknown-linux-gnu.tar.gz) ([checksum](https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-x86_64-unknown-linux-gnu.tar.gz.sha256sum))
- [Linux ARM64](https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-aarch64-unknown-linux-gnu.tar.gz) ([checksum](https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/$CHANNEL/$VERSION/btc-server-aarch64-unknown-linux-gnu.tar.gz.sha256sum))

### Docker Images

#### BTC Server
\`\`\`bash
docker pull ghcr.io/botanix-labs/botanix-btc-server:$VERSION
docker pull ghcr.io/botanix-labs/botanix-btc-server:$CHANNEL
\`\`\`

#### Reth Node
\`\`\`bash  
docker pull ghcr.io/botanix-labs/botanix-reth-node:$VERSION
docker pull ghcr.io/botanix-labs/botanix-reth-node:$CHANNEL
\`\`\`

## Verification

### Binary Checksums
\`\`\`bash
# Download and verify checksums
wget https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz
wget https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz.sha256sum
sha256sum -c reth-x86_64-unknown-linux-gnu.tar.gz.sha256sum
\`\`\`

### Docker Image Verification
\`\`\`bash
# Inspect image labels
docker inspect ghcr.io/botanix-labs/botanix-btc-server:$VERSION --format='{{.Config.Labels}}'
\`\`\`

## Installation

### Quick Start with Docker
\`\`\`bash
# Run BTC Server
docker run -d --name botanix-btc-server \\
  -p 8080:8080 \\
  ghcr.io/botanix-labs/botanix-btc-server:$VERSION

# Run Reth Node  
docker run -d --name botanix-reth-node \\
  -p 30303:30303 -p 8545:8545 \\
  ghcr.io/botanix-labs/botanix-reth-node:$VERSION
\`\`\`

### Binary Installation
\`\`\`bash
# Download and extract
wget https://storage.googleapis.com/botanix-artifact-registry/releases/reth/$CHANNEL/$VERSION/reth-x86_64-unknown-linux-gnu.tar.gz
tar -xzf reth-x86_64-unknown-linux-gnu.tar.gz
sudo mv reth /usr/local/bin/
\`\`\`

## Previous Releases

See [all releases](../../README.md#releases) for version history.
EOF
    
    echo "✅ Generated release README"
}

update_changelog() {
    echo "Updating changelog for $CHANNEL channel..."
    
    local CHANGELOG_FILE="botanix/changelog/$CHANNEL/CHANGELOG.md"
    
    if [ ! -f "$CHANGELOG_FILE" ]; then
        cat > "$CHANGELOG_FILE" << EOF
# Botanix $CHANNEL Channel Changelog

All notable changes to the $CHANNEL release channel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

EOF
    fi
    
    {
        head -n 6 "$CHANGELOG_FILE"
        echo ""
        echo "## [$VERSION] - $(date -u +%Y-%m-%d)"
        echo ""
        echo "$RELEASE_NOTES"
        echo ""
        echo "**Downloads:** [Release Page](../../releases/$VERSION/)"
        echo ""
        tail -n +7 "$CHANGELOG_FILE"
    } > "$CHANGELOG_FILE.tmp"
    
    mv "$CHANGELOG_FILE.tmp" "$CHANGELOG_FILE"
    echo "✅ Updated changelog"
}

update_release_index() {
    echo "Updating release index..."
    
    local INDEX_FILE="botanix/releases/index.json"
    
    if [ ! -f "$INDEX_FILE" ]; then
        echo '{"releases":[],"channels":{},"latest":{}}' > "$INDEX_FILE"
    fi
    
    local prerelease_flag=$([ "$CHANNEL" != "stable" ] && echo "true" || echo "false")
    
    jq --arg version "$VERSION" \
       --arg channel "$CHANNEL" \
       --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
       --arg prerelease "$prerelease_flag" \
       '.releases += [{"version": $version, "channel": $channel, "date": $date, "prerelease": ($prerelease == "true"), "path": "releases/" + $version}] | .channels[$channel] = $version | if $channel == "stable" then .latest.stable = $version else .latest[$channel] = $version end | .releases |= sort_by(.date) | reverse' \
       "$INDEX_FILE" > "$INDEX_FILE.tmp"
    
    mv "$INDEX_FILE.tmp" "$INDEX_FILE"
    echo "✅ Updated release index"
}

update_public_readme() {
    echo "Updating public repository README..."
    
    cat > "botanix/README.md" << 'EOF'
# Botanix Public Releases

This repository contains public release artifacts, documentation, and changelogs for Botanix.

## Latest Releases

| Channel | Version | Release Date | Downloads |
|---------|---------|--------------|-----------|
EOF
    
    # Add release information from index
    if [[ -f "botanix/releases/index.json" ]]; then
        jq -r '.channels | to_entries[] | "| " + .key + " | " + .value + " | | [Download](releases/" + .value + ") |"' botanix/releases/index.json >> botanix/README.md
    fi
    
    cat >> "botanix/README.md" << 'EOF'

## Quick Start

### Docker (Recommended)
```bash
# Latest stable release
docker pull ghcr.io/botanix-labs/botanix-reth-node:latest
docker pull ghcr.io/botanix-labs/botanix-btc-server:latest

# Development builds
docker pull ghcr.io/botanix-labs/botanix-reth-node:alpha
docker pull ghcr.io/botanix-labs/botanix-btc-server:alpha
```

### Binary Installation
```bash
# Download latest stable binaries
curl -L https://storage.googleapis.com/botanix-artifact-registry/releases/reth/stable/latest/reth-x86_64-unknown-linux-gnu.tar.gz | tar -xz
curl -L https://storage.googleapis.com/botanix-artifact-registry/releases/btc-server/stable/latest/btc-server-x86_64-unknown-linux-gnu.tar.gz | tar -xz
```

## Documentation

- [Release Notes](releases/) - Detailed release information
- [Changelogs](changelog/) - Version history by channel

## Channels

- **stable** (latest) - Production-ready releases
- **rc** - Release candidates for testing
- **alpha** - Development builds with latest features
- **hotfix** - Emergency fixes for production issues

## Support

- [GitHub Issues](https://github.com/botanix-labs/botanix-releases/issues)
- [Documentation](https://github.com/botanix-labs/documentation)

---

*This repository is automatically updated by the release pipeline.*
EOF
    
    echo "✅ Updated public README"
}

commit_and_push_changes() {
    echo "Committing and pushing changes to public repository..."
    
    cd botanix
    git add .
    
    if git diff --staged --quiet; then
        echo "No changes to commit"
        return 0
    else
        git commit -m "feat: release v$VERSION

- Add release artifacts and documentation
- Update changelogs and release index
- Generated from botanix-labs/macbeth@$GIT_SHA

🤖 Automated release by GitHub Actions"
        git push origin main
        echo "✅ Successfully published release v$VERSION to public repository"
    fi
}

case "$COMMAND" in
    "manifest")
        generate_release_manifest
        ;;
    "readme")
        generate_release_readme
        ;;
    "changelog")
        update_changelog
        ;;
    "index")
        update_release_index
        ;;
    "public-readme")
        update_public_readme
        ;;
    "commit")
        commit_and_push_changes
        ;;
    "full-publish")
        generate_release_manifest
        generate_release_readme
        update_changelog
        update_release_index
        update_public_readme
        commit_and_push_changes
        ;;
    *)
        echo "Usage: $0 <manifest|readme|changelog|index|public-readme|commit|full-publish> <version> <channel> <release_notes> [git_tag] [git_sha] [prev_version]"
        exit 1
        ;;
esac