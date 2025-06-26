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

# Back-merge changes from main to other branches according to release strategy
back_merge() {
  local SOURCE_BRANCH="$1"
  local TARGET_BRANCH="$2"
  local GITHUB_TOKEN="$3"

  if [ -z "$SOURCE_BRANCH" ] || [ -z "$TARGET_BRANCH" ]; then
    log_error "Source and target branches are required for back-merge"
  fi

  log_info "Back-merging from $SOURCE_BRANCH to $TARGET_BRANCH"

  # Ensure we have the latest from remote
  git fetch origin

  # Check if target branch exists
  if git ls-remote --heads origin "$TARGET_BRANCH" | grep -q "$TARGET_BRANCH"; then
    # Create a temporary branch for the back-merge
    local TEMP_BRANCH="back-merge/${SOURCE_BRANCH}-to-${TARGET_BRANCH}"
    local TIMESTAMP=$(date +"%Y%m%d%H%M%S")
    TEMP_BRANCH="${TEMP_BRANCH}-${TIMESTAMP}"

    log_info "Creating temporary branch: $TEMP_BRANCH"

    # Delete branch if it already exists locally
    git branch -D "$TEMP_BRANCH" 2>/dev/null || true

    # Start with the target branch
    git checkout -b "$TEMP_BRANCH" "origin/${TARGET_BRANCH}"

    # Try to merge, but if it fails, create a PR instead
    if git merge --no-ff "origin/${SOURCE_BRANCH}" -m "chore(release): back-merge from $SOURCE_BRANCH to $TARGET_BRANCH" 2>/dev/null; then
      log_info "No conflicts detected, performing direct merge"

      # No conflicts, push directly
      git push origin "$TEMP_BRANCH":"$TARGET_BRANCH"
      log_info "Successfully back-merged from $SOURCE_BRANCH to $TARGET_BRANCH"

      # Clean up temporary branch
      git checkout "$SOURCE_BRANCH" 2>/dev/null || git checkout "origin/$SOURCE_BRANCH"
      git branch -D "$TEMP_BRANCH"
    else
      # Merge conflict detected, abort merge and create PR
      log_warn "Merge conflicts detected when back-merging from $SOURCE_BRANCH to $TARGET_BRANCH"
      git merge --abort

      # Identify conflicting files for better PR description
      MERGE_BASE=$(git merge-base "origin/${SOURCE_BRANCH}" "origin/${TARGET_BRANCH}")
      CONFLICT_FILES=$(git diff --name-only "$MERGE_BASE" "origin/${SOURCE_BRANCH}" "origin/${TARGET_BRANCH}" | sort | uniq -d)
      CONFLICT_COUNT=$(echo "$CONFLICT_FILES" | grep -v '^$' | wc -l)

      log_info "Found $CONFLICT_COUNT potentially conflicting files"

      # Create a new branch from target branch
      git checkout -b "$TEMP_BRANCH" "origin/${TARGET_BRANCH}"

      # Create a merge commit that will be the basis for the PR
      echo "# Back-merge from $SOURCE_BRANCH to $TARGET_BRANCH\n\nThis branch contains changes from $SOURCE_BRANCH that need to be merged into $TARGET_BRANCH." > BACK_MERGE_MESSAGE.md
      git add BACK_MERGE_MESSAGE.md
      git commit -m "chore(release): prepare back-merge from $SOURCE_BRANCH to $TARGET_BRANCH"

      # Push the branch to remote
      git push origin "$TEMP_BRANCH"

      log_info "Creating pull request for manual resolution"

      # Extract repository info from git remote
      local REPO_URL=$(git remote get-url origin)
      local REPO_PATH=$(echo "$REPO_URL" | sed -n 's/.*github\.com[:\/]\([^\.]*\).*/\1/p')

      # Create PR if GitHub token is provided
      if [ -n "$GITHUB_TOKEN" ]; then
        PR_TITLE="chore(release): back-merge from $SOURCE_BRANCH to $TARGET_BRANCH"

        # Create a more detailed PR description with conflict information
        PR_BODY="## Automated Back-merge PR\n\nThis PR was automatically created to back-merge changes from \`$SOURCE_BRANCH\` to \`$TARGET_BRANCH\`.\n\n### ⚠️ Merge conflicts detected!\n\n"

        if [ "$CONFLICT_COUNT" -gt 0 ]; then
          PR_BODY+="#### Potential conflict files:\n\n"
          while IFS= read -r file; do
            if [ -n "$file" ]; then
              PR_BODY+="- \`$file\`\n"
            fi
          done <<< "$CONFLICT_FILES"
          PR_BODY+="\n"
        else
          PR_BODY+="Unable to determine specific conflicting files. Please review the PR carefully.\n\n"
        fi

        PR_BODY+="### Steps to resolve\n\n1. Checkout this branch: \`git checkout $TEMP_BRANCH\`\n2. Merge target branch: \`git merge origin/$TARGET_BRANCH\`\n3. Resolve conflicts manually\n4. Commit and push: \`git commit -m 'chore: resolve back-merge conflicts' && git push\`\n\nOnce all conflicts are resolved, this PR can be merged to complete the back-merge operation."

        # Create the PR using GitHub API
        PR_RESPONSE=$(curl -s -X POST \
          -H "Authorization: token $GITHUB_TOKEN" \
          -H "Accept: application/vnd.github.v3+json" \
          "https://api.github.com/repos/$REPO_PATH/pulls" \
          -d '{"title":"'"$PR_TITLE"'","body":"'"$PR_BODY"'","head":"'"$TEMP_BRANCH"'","base":"'"$TARGET_BRANCH"'"}' || echo "Failed to create PR")

        # Extract PR URL and number if successful
        if echo "$PR_RESPONSE" | grep -q "html_url"; then
          PR_URL=$(echo "$PR_RESPONSE" | grep -o '"html_url": "[^"]*"' | head -1 | cut -d '"' -f 4)
          PR_NUMBER=$(echo "$PR_RESPONSE" | grep -o '"number": [0-9]*' | head -1 | cut -d ':' -f 2 | tr -d ' ')

          log_info "Pull request #$PR_NUMBER created for manual resolution: $PR_URL"

          # Add labels to the PR
          curl -s -X POST \
            -H "Authorization: token $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/$REPO_PATH/issues/$PR_NUMBER/labels" \
            -d '{"labels":["back-merge", "conflicts", "release-automation"]}' > /dev/null

          # Try to assign reviewers (repo owners or maintainers)
          MAINTAINERS_RESP=$(curl -s -X GET \
            -H "Authorization: token $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/$REPO_PATH/collaborators?permission=maintain" | grep -o '"login": "[^"]*"' | head -3 | cut -d '"' -f 4 | tr '\n' ',')

          if [ -n "$MAINTAINERS_RESP" ]; then
            MAINTAINERS="[\"${MAINTAINERS_RESP%,}\"]"
            MAINTAINERS=$(echo "$MAINTAINERS" | sed 's/,/\",\"/g')

            curl -s -X POST \
              -H "Authorization: token $GITHUB_TOKEN" \
              -H "Accept: application/vnd.github.v3+json" \
              "https://api.github.com/repos/$REPO_PATH/pulls/$PR_NUMBER/requested_reviewers" \
              -d "{\"reviewers\":$MAINTAINERS}" > /dev/null
          fi

          # Create a status check to indicate this is an automated PR
          curl -s -X POST \
            -H "Authorization: token $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/$REPO_PATH/statuses/$(git rev-parse HEAD)" \
            -d '{"state":"success","context":"Back-merge Automation","description":"This branch was created for automated back-merging"}' > /dev/null

          # If the repo has GitHub Projects, add the PR to the project
          PROJECTS_RESP=$(curl -s -X GET \
            -H "Authorization: token $GITHUB_TOKEN" \
            -H "Accept: application/vnd.github.v3+json" \
            "https://api.github.com/repos/$REPO_PATH/projects" | grep -o '"id": [0-9]*' | head -1 | cut -d ':' -f 2 | tr -d ' ')

          if [ -n "$PROJECTS_RESP" ]; then
            log_info "Adding PR to project board"
            curl -s -X POST \
              -H "Authorization: token $GITHUB_TOKEN" \
              -H "Accept: application/vnd.github.v3+json" \
              "https://api.github.com/projects/columns/cards" \
              -d '{"content_id":"'"$PR_NUMBER"'","content_type":"PullRequest"}' > /dev/null || true
          fi
        else
          log_error "Failed to create pull request: $PR_RESPONSE"
        fi
      else
        log_warn "No GitHub token provided, cannot create PR automatically."
        log_info "Please create PR manually from branch '$TEMP_BRANCH' to '$TARGET_BRANCH'"
      fi
    fi
  else
    log_warn "Target branch $TARGET_BRANCH does not exist, skipping back-merge"
  fi

  # Return to original branch
  git checkout "$SOURCE_BRANCH"
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
  echo "Usage: release.sh COMMAND [ARGS...]"
  echo
  echo "Commands:"
  echo "  update VERSION          Update version in Cargo.toml"
  echo "  tag VERSION             Create a git tag for the release"
  echo "  back_merge SRC DST      Back-merge from source branch to destination branch"
  echo "  help                    Show this help message"
  echo
  echo "Examples:"
  echo "  release.sh update 1.2.3        Update version to 1.2.3"
  echo "  release.sh tag 1.2.3           Create tag v1.2.3"
  echo "  release.sh back_merge main rc  Back-merge from main to rc branch"
}


case "$COMMAND" in
  update)
    update_cargo_version
    ;;
  tag)
    create_tag
    ;;
  back_merge)
    back_merge "$2" "$3" "$4"
    ;;
  multi_back_merge)
    # Handle back-merging from one branch to multiple target branches
    SOURCE_BRANCH="$2"
    GITHUB_TOKEN="$3"
    shift 3
    TARGET_BRANCHES="$@"

    if [ -z "$SOURCE_BRANCH" ] || [ -z "$TARGET_BRANCHES" ]; then
      log_error "Source branch and at least one target branch are required"
    fi

    log_info "Performing back-merge from $SOURCE_BRANCH to multiple branches: $TARGET_BRANCHES"

    for target in $TARGET_BRANCHES; do
      log_info "Processing target: $target"
      back_merge "$SOURCE_BRANCH" "$target" "$GITHUB_TOKEN"
    done
    ;;
  help)
    show_help
    ;;
  *)
    log_error "Unknown command: $COMMAND. Use: update, tag, back_merge, multi_back_merge, or help"
    ;;
esac

exit 0
