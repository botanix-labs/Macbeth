# Release Strategy

This document describes our release strategy, including branch management and versioning.
Only code owners should these actions.

## Branch flow

```
  ┌───────────────────────────────┐
  │             main              │  v1.0.0, v1.1.0, etc.
  └───────────────┬───────────────┘
                  │            ▲ 
                  │            │ 
                  ▼            │ 
  ┌────────────────────────┐   │ 
  │         hotfix          │   │     v1.0.1-hotfix.1
  └───────────┬────────────┘   │
              │                │
              │                │
              │                │
              │      ┌─────────┘
              │      │        
              ▼      ▼                 
  ┌────────────────────────┐           
  │          rc            ├───┐     v1.1.0-rc.1, v1.1.0-rc.2
  └───────────┬────────────┘   │
              │         ▲      │
              │         │      │
              │         │      │
              ▼         │      │
  ┌────────────────────────┐   │
  │          dev           │   │     v1.1.0-beta.1, v1.1.0-beta.2
  └───────────┬────────────┘   │
              │         ▲      │
              │         │      │
              ▼         │      │
  ┌────────────────────────┐   │
  │       feat/...         ├───┘     A new PR branch
  └────────────────────────┘
```

## Semantic Versioning

We follow [Semantic Versioning 2.0.0](https://semver.org/) with the format `MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]`:

- **MAJOR**: Incremented for incompatible user-facing API changes
- **MINOR**: Incremented for backward-compatible functionality additions
- **PATCH**: Incremented for backward-compatible bug fixes
- **PRERELEASE**: Optional identifier for pre-releases (e.g., -beta.1, -rc.2, -hotfix.1)
- **BUILD**: Optional build metadata (e.g., +20230415)

## Branches

### main
- Contains the latest stable version of the code
- Production-ready and can be deployed at any time
- Can only receive merges from `hotfix` or `rc` branches
- Each merge to `main` gets tagged with a version number and triggers a release
- After merging to `main`, changes must be back-merged to `hotfix`, `develop`, and `rc`

### hotfix
- Created from `main`
- Used for urgent fixes to production code
- Only bug fixes can be merged (branches named `fix/...`)
- Bumps only PATCH version from current `main` version
- Adds `-hotfix.N` pre-release segment (e.g., v1.0.1-hotfix.1)
- No breaking changes allowed
- Can be merged directly to `main` after testing

### rc (Release Candidate)
- Used for testing release candidates before production
- Can only receive merges from `develop`
- Adds `-rc.N` pre-release segment (e.g., v1.1.0-rc.1)
- After testing is successful, can be merged to `main`
- If issues are found, fixes are made in `develop` and merged to `rc`
- In case if `develop` already contains next release changes, fixes can be merged directly to `rc`

### develop
- Integration branch for the next release
- Contains all new features, bug fixes, and other changes
- No breaking changes allowed, except for a major release planned
- Releases with `-beta.N` pre-release segment (e.g., v1.1.0-beta.1)
- Version bumped based on changes:
  - New features: MINOR version bump
  - Other changes: PATCH version bump

## Release Processes

### Stable Release
1. Merge `hotfix` or `develop` branches into `main`
2. Bump package package versions (e.g., v1.1.0)
3. Update change log
4. Back-merge changes to `develop`,
5. Force-merge `main` to `rc` and `hotfix`
6. Tag the latest commit in `main` with the version number (e.g., v1.1.0)
7. Build artifacts
8. Push artifacts

### Hotfix Release
1. Bump package package versions (e.g., v1.0.1-hotfix.1)
2. Update change log
3. Tag the `hotfix` branch with version (e.g., v1.0.1-hotfix.1)
4. Build artifacts
5. Push artifacts

### Beta Release
1. Bump package package versions (e.g., v1.1.0-beta.1)
2. Update change log
3. Tag `develop` with a beta version (e.g., v1.1.0-beta.1)
4. Build artifacts
5. Push artifacts 

### Release Candidate Release
1. Merge `develop` branch into `rc`
2. Bump package package versions (e.g., v1.1.0-rc.1)
3. Update change log
4. Tag the `rc` branch with RC version (e.g., v1.1.0-rc.1)
5. Build artifacts
6. Push artifacts

## Change Log

Release notes should include:

- Version number and release date
- Summary of changes
- New features with documentation links
- List of changes
- Upgrade instructions if needed
- Contributors acknowledgment

## Release Frequency

- Stable releases: Every 2-4 weeks
- Beta releases: Weekly during active development
- Release candidates: As needed
- Hotfixes: As needed, deployed as soon as tested
