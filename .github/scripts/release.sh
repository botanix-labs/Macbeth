#!/usr/bin/env bash

# Script for managing releases in the Macbeth Codebase

set -e

COMMAND="$1"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# Cargo.toml main file
CARGO_TOML="Cargo.toml"

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
    log_error "Command is required. Use: update_cargo_version, commit_cargo_files, back_merge, or help"
fi

# Update version in Cargo.toml workspace file
update_cargo_version() {
    local VERSION="$1"

    if [ -z "$VERSION" ]; then
        log_error "Version is required for update_cargo_version command"
    fi

    log_info "Updating Cargo.toml version to $VERSION"

    # Use sed to replace the version line only in [workspace.package] section
    sed -i '/^\[workspace\.package\]/,/^\[/{s/^version = .*$/version = "'"$VERSION"'"/}' "$CARGO_TOML"

    if ! grep -q "version = \"$VERSION\"" "$CARGO_TOML"; then
        log_error "Failed to update version in $CARGO_TOML"
    fi

    cargo generate-lockfile

    log_info "Successfully updated version to $VERSION in $CARGO_TOML"
}

# Back-merge changes from main to other branches according to release strategy
back_merge() {
    local SOURCE_BRANCH="$1"
    local TARGET_BRANCH="$2"
    local GITHUB_TOKEN="$3"
    local VERSION="$4"

    if [ -z "$SOURCE_BRANCH" ] || [ -z "$TARGET_BRANCH" ]; then
        log_error "Source and target branches are required for back-merge"
    fi

    log_info "Back-merging from $SOURCE_BRANCH to $TARGET_BRANCH"

    # Ensure we have the latest from remote
    git fetch origin

    # Create a temporary branch for the back-merge
    local TEMP_BRANCH="back-merge/${SOURCE_BRANCH}-to-${TARGET_BRANCH}"
    local TIMESTAMP=$(date +"%Y%m%d%H%M%S")
    TEMP_BRANCH="${TEMP_BRANCH}-${TIMESTAMP}"

    log_info "Creating temporary branch: $TEMP_BRANCH"

    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"

    # Delete branch if it already exists locally
    git branch -D "$TEMP_BRANCH" 2> /dev/null || true

    # Start with the target branch
    git checkout -b "$TEMP_BRANCH" "origin/${TARGET_BRANCH}"

    # Try to merge, but if it fails, create a PR instead
    set +e
    MERGE_OUTPUT=$(git merge --no-ff "origin/${SOURCE_BRANCH}" -m "chore(release): back-merge v$VERSION from $SOURCE_BRANCH to $TARGET_BRANCH" 2>&1)
    MERGE_EXIT_CODE=$?
    set -e

    if [ $MERGE_EXIT_CODE -eq 0 ]; then
        log_info "No conflicts detected, performing direct merge"

        # No conflicts, push directly
        git push origin "$TEMP_BRANCH":"$TARGET_BRANCH"
        log_info "Successfully back-merged from $SOURCE_BRANCH to $TARGET_BRANCH"
    else
        # Merge conflict detected, abort merge and create PR
        log_warn "Automatic back merge failed: $MERGE_OUTPUT"

        git merge --abort || true

        # Delete temp from made from target
        git checkout "$SOURCE_BRANCH"
        git branch -D "$TEMP_BRANCH"

        # Create a new temp branch made from source
        git checkout -b "$TEMP_BRANCH" "origin/${SOURCE_BRANCH}"

        log_info "Creating pull request for manual resolution"

        git push origin "$TEMP_BRANCH"

        # Extract repository info from git remote
        local REPO_URL=$(git remote get-url origin)
        local REPO_PATH=$(echo "$REPO_URL" | sed -n 's/.*github\.com[:\/]\([^\.]*\).*/\1/p')

        # Create PR if GitHub token is provided
        PR_TITLE="chore(release): back-merge v$VERSION from $SOURCE_BRANCH to $TARGET_BRANCH"

        # Create a more detailed PR description with conflict information
        PR_BODY="## Automated Back-merge PR\n\nThis PR was automatically created to back-merge changes from \`$SOURCE_BRANCH\` to \`$TARGET_BRANCH\`.\n\n### ⚠️ Merge conflicts detected!"
        PR_BODY+="### Steps to resolve\n\n1. Checkout this branch: \`git checkout $TEMP_BRANCH\`\n2. Merge target branch: \`git merge origin/$TARGET_BRANCH\`\n3. Resolve conflicts manually\n4. Commit and push: \`git commit -m 'chore(release): resolve back-merge conflicts' && git push\`\n\nOnce all conflicts are resolved, this PR can be merged to complete the back-merge operation."

        # Create the PR using GitHub API
        curl -s -X POST \
            -H "Authorization: token $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/$REPO_PATH/pulls" \
            -d '{"title":"'"$PR_TITLE"'","body":"'"$PR_BODY"'","head":"'"$TEMP_BRANCH"'","base":"'"$TARGET_BRANCH"'"}' || echo "Failed to create PR"
    fi

    # Return to original branch
    git reset --hard HEAD
    git checkout "$SOURCE_BRANCH"
}

# Commit Cargo.toml and Cargo.lock files with version bump
commit_cargo_files() {
    local VERSION="$1"

    if [ -z "$VERSION" ]; then
        log_error "Version is required for commit_cargo_files command"
    fi

    log_info "Committing Cargo.toml and Cargo.lock for version $VERSION"

    git config --global user.name "github-actions[bot]"
    git config --global user.email "github-actions[bot]@users.noreply.github.com"
    git add Cargo.toml Cargo.lock
    git commit -m "chore(release): bump version to $VERSION"

    log_info "Successfully committed version bump to $VERSION"
}

show_help() {
    echo "Usage: release.sh COMMAND [ARGS...]"
    echo
    echo "Commands:"
    echo "  update_cargo_version VERSION       Update version in Cargo.toml"
    echo "  commit_cargo_files VERSION         Commit Cargo.toml and Cargo.lock with version bump"
    echo "  back_merge SRC DST TOKEN VERSION   Back-merge from source branch to destination branch"
    echo "  help                                Show this help message"
    echo
    echo "Examples:"
    echo "  release.sh update_cargo_version 1.2.3                     Update version to 1.2.3"
    echo "  release.sh commit_cargo_files 1.2.3                       Commit version bump files"
    echo "  release.sh back_merge main rc TOKEN 1.2.3                 Back-merge from main to rc branch"
}

case "$COMMAND" in
    update_cargo_version)
        update_cargo_version "$2"
        ;;
    commit_cargo_files)
        commit_cargo_files "$2"
        ;;
    back_merge)
        back_merge "$2" "$3" "$4" "$5"
        ;;
    help)
        show_help
        ;;
    *)
        log_error "Unknown command: $COMMAND. Use: update_cargo_version, commit_cargo_files, back_merge, or help"
        ;;
esac

exit 0
