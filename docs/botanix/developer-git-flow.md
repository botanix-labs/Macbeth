# Developer Git Workflow

This document describes our Git workflow from a developer perspective: branch-topology,
naming conventions, commit messages, and the pull‑request life‑cycle.

## Branches

```
  ┌──────────┐    ┌───────────┐
  │  hotfix  │    |  develop  │
  └──────────┘    └───────────┘
       ▲                ▲
       │                │
  ┌────┴────────────────┴─────┐
  │   <type>/[scope]/<name>   │
  └───────────────────────────┘
```

- `hotfix` - The latest stable version. This branch is used to test hotfixes before release. Only critical bug fixes should be merged into this branch.
- `develop` - Integration branch for a new release. It contains all the new features and bug fixes.
- `<type>/[scope]/<name>` - A new change branch that should be created and merged back to `hotfix` or `develop`.

All changes to `hotfix` and `develop` branches should be done through pull requests (PRs).

To choose the base branch for your PR, consider the following:

- If you are working on a new feature or bug fix for a future version, create a new branch from `develop`.
- If you are fixing a bug in the latest stable version, create a new branch from `hotfix`.

New branch name must have a format of `<type>/[scope]/<name>` and follow [commit rules](#commits).
The `name` must be alphanumeric and can contain dashes. It should be descriptive enough to understand the purpose of the branch.

**Examples:**

- `feat/implement-attestation` - Adding a new attestation feature
- `fix/pegin/handle-transaction-error` - Fixing an error in the pegin transaction handling
- `docs/update-quickstart` - Updating the quickstart section in the readme

## Commits

When you have a new branch ready, it's time to make and commit your changes.

Commit messages must follow [the conventional commits](https://www.conventionalcommits.org/en/v1.0.0/) specification:

```
<type>[scope][!]: <description>

[optional body]

[optional footer(s)]
```

- `type` is the type of change:
    - `feat` - a new business feature
    - `chore` - a developer feature. For example, updating dependencies, adding logs, etc.
    - `fix` - a bug fix
    - `build` - changes that affect the build system or external dependencies
    - `ci` - changes to our CI configuration files and scripts
    - `docs` - documentation only changes
    - `style` - changes that do not affect the meaning of the code (white-space, formatting, missing semi-colons, etc)
    - `refactor` - a code refactoring
    - `perf` - a code change that improves performance
    - `test` - adding missing tests or correcting existing tests
    - `revert` - reverting a commit
- `!` is used to indicate a breaking change:
    - any user-facing API has incompatible changes
    - previously created data is no longer valid
    - any other change that requires users to take action
- `scope` is the area of the codebase affected. Must be a crate name where changes are located.
  Skip if changes affect multiple crates or changes are done outside of crates.
- `description` is a short summary of the change

Commit message title should not be longer than 50 characters.

**Examples:**

```
feat(consensus): implement attestation verification
fix(pegin): handle transaction validation errors
docs: update installation instructions
refactor!(api): change response format for transaction endpoints
```

Changes in a commit must be atomic and focused:

- Each commit should contain a single logical change
- Do not mix different types of changes (e.g., refactoring and new features) in the same commit
- Do not include multiple features in a single commit

Commits must be signed with a PGP key. See [GitHub's documentation on signing commits](https://docs.github.com/en/authentication/managing-commit-signature-verification/signing-commits) for more information.

Frequently commit and push your changes to ensure your work is backed up and visible to other team members.

## Pull requests

1. PR title must follow [commit rules](#commits).
2. Changes in a PR must be atomic and focused:
    - Each PR should contain a single logical change
    - Do not mix different types of changes (e.g., refactoring and new features) in the same PR
    - Do not include multiple features in a single PR
    - Keep PRs as small as possible to facilitate review and reduce merge conflicts
    - Changes in a PR must be complete and ready to be released (do not break existing functionality)
3. PR description must contain:
    - An issue that this PR is solving:
        - "A user can't pegin N amount of bitcoin and receives an error"
        - "We need this feature to be able to do X"
    - A generalized list of changes
    - How this PR was tested
    - Breaking changes, if any
4. The PR should be marked as a draft if it's under development.
5. The PR must be assigned to at least one responsible developer ("Assignees").
6. The PR must be assigned to at least one reviewer ("Reviewers").
7. The PR must be added to the project board ("Project") or linked to an issue from the board ("Development").

### Example PR description

```markdown
## Issue being fixed or feature implemented
Users cannot pegin amounts less than 0.001 BTC due to a validation error in the transaction handling.

## What was done?
- Fixed the validation logic in the pegin module
- Added proper error handling for small amounts
- Updated the user-facing error message

## How Has This Been Tested?
- Added unit tests for various pegin amounts
- Manually tested with 0.0005 BTC pegin transactions
- Verified error messages are clear and helpful

## Breaking Changes
None

## Checklist:
- [x] I have added or updated relevant unit/integration/functional/e2e tests
- [x] I have tested my changes running a local network, and they work as expected
- [x] I have performed a self-review
- [x] I have commented my code, particularly in hard-to-understand areas
- [x] I have added "!" to the title and described breaking changes in the corresponding section if my code contains any
- [x] I have made corresponding changes to the documentation if needed
```

### Checks

1. All new code is covered by tests and/or existing tests are updated
2. Documentation is updated if needed
3. Code has enough comments to understand the logic
4. The PR is ready for review
5. Self-review has been completed
6. The change is tested on a local network and works as expected
7. All tests are passing
   * Rust unit tests
   * Rust integration tests
   * Minting contract tests
8. Code is formatted
9. Code is linted
   * Cargo check
   * Grafana a dashboard JSON structure
   * Cargo lock file
   * GitHub actions
   * Misspellings
   * Clippy warnings
10. Pre-review with AI is done
11. At least two code owners (team member(s) responsible for codebase) have approved the PR
12. The PR has not been rejected by any reviewer
13. All comments have been resolved. Comments must be resolved by a person who made the comment, except for AI-generated comments.

### Merging

Merging must be available only when all [checks](#checks) are passed.

The PR should be merged into the base branch using the ["Squash and merge"](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/incorporating-changes-from-a-pull-request/about-pull-request-merges#squash-and-merge-your-pull-request-commits) option. This will create a single commit with all changes from the PR and keep the commit history in integration branches clean.

Commit message should be equal to PR title plus PR number. This will allow us to keep a reference to all PR information, including squashed commits.

**Example Squash Commit Message:**

```
feat(consensus): implement attestation verification (#1234)
```
