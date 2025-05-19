#!/usr/bin/env bash

# Script for managing releases in the Macbeth Codebase

set -e


COMMAND="$1"

VERSION="$2"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# Cargo.toml main file
CARGO_TOML="../../Cargo.toml"

# Log messages with color
log_info() {
  echo -e "${GREEN}INFO: $1${NC}"
}

log_warn() {
  echo -e "${YELLOW}WARNING: $1${NC}"
}

log_error() {
  echo -e "${RED}ERROR: $1${NC}"
  exit 1
}

if [ -z "$COMMAND" ]; then
  log_error "Command is required. Use: update, tag, or help"
fi


# Update version in Cargo.toml workspace file
update_cargo_version() {
  if [ -z "$VERSION" ]; then
    log_error "Version is required for update command"
  fi
  
  log_info "Updating Cargo.toml version to $VERSION"
  
  # Use sed to replace the version line in Cargo.toml
  sed -i "s/^version = .*$/version = \"$VERSION\"/" "$CARGO_TOML"
  
  if ! grep -q "version = \"$VERSION\"" "$CARGO_TOML"; then
    log_error "Failed to update version in $CARGO_TOML"
  fi
  
  log_info "Successfully updated version to $VERSION in $CARGO_TOML"
  
  git add "$CARGO_TOML"
  
  git commit -m "chore(release): bump version to ${VERSION} [skip ci]" || true
}


# Create a git tag for the release
create_tag() {
  if [ -z "$VERSION" ]; then
    log_error "Version is required for tag command"
  fi
  
  TAG="v$VERSION"
  
  # Check if tag already exists
  if git tag -l | grep -q "^$TAG$"; then
    log_error "Tag $TAG already exists"
  fi
  
  log_info "Creating tag $TAG"
  git tag -a "$TAG" -m "Release $TAG"
  log_info "Tag $TAG created"
}


show_help() {
  echo "Usage: release.sh COMMAND [VERSION]"
  echo
  echo "Commands:"
  echo "  update VERSION   Update version in Cargo.toml"
  echo "  tag VERSION      Create a git tag for the release"
  echo "  help             Show this help message"
  echo
  echo "Examples:"
  echo "  release.sh update 1.2.3   Update version to 1.2.3"
  echo "  release.sh tag 1.2.3      Create tag v1.2.3"
}


case "$COMMAND" in
  update)
    update_cargo_version
    ;;
  tag)
    create_tag
    ;;
  help)
    show_help
    ;;
  *)
    log_error "Unknown command: $COMMAND. Use: update, tag, or help"
    ;;
esac

exit 0