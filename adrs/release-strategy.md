# Release Strategy

This document describes our release strategy: release branch-topology, branch management, versioning, and release schedule.

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
  │        develop         │   │     v1.1.0-alpha.1-a30ddd1, v1.1.0-alpha.2-a30ddd1
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
- **PRERELEASE**: Optional identifier for pre-releases (e.g., `-beta.1`, `-rc.2`, `-hotfix.1`)
- **BUILD**: Optional build commit SHA (e.g., `+a30ddd1`)

## Branches

### main
- Contains the latest stable version of the code
- Production-ready and can be deployed at any time
- Can only receive merges from `hotfix` or `rc` branches
- Only code owners should be able to merge to `main`
- Each merge to `main` gets tagged with a version number and triggers a release
- After merging to `main`, changes must be back-merged to `hotfix`, `develop`, and `rc`

### hotfix
- Created from `main`
- Used for urgent fixes to production code
- Only bug fixes can be merged (branches named `fix/...`)
- Bumps only PATCH version from current `main` version
- Adds `-hotfix.N` pre-release segment (e.g., `v1.0.1-hotfix.1`)
- No breaking changes allowed
- Can be merged directly to `main` after testing

### rc (Release Candidate)
- Used for testing release candidates before production
- Can only receive merges from `develop`
- Only code owners should be able to merge to `rc`
- Adds `-rc.N` pre-release segment (e.g., `v1.1.0-rc.1`)
- After testing is successful, can be merged to `main`
- If issues are found, fixes are made in `develop` and merged to `rc`
- In case if `develop` already contains next release changes, fixes can be merged directly to `rc`

### develop
- Integration branch for the next release
- Contains all new features, bug fixes, and other changes
- No breaking changes allowed, except for a major release planned
- Releases with `-alpha.N-SHA` pre-release segment and build commit SHA (e.g., `v1.1.0-alpha.1-a30ddd1`)
- Version bumped based on changes:
  - New features: `MINOR` version bump
  - Other changes: `PATCH` version bump

## Release Process

### Stable Release

1. Merge `hotfix` or `rc` branches into `main`
2. Bump package versions (e.g., `v1.1.0`)
3. Update change log
4. Back-merge changes to `develop`,
5. Force-merge `main` to `rc` and `hotfix`
6. Create release on GitHub from `main` branch with the version number (e.g., `v1.1.0`).
7. Build artifacts
8. Push artifacts

### Release Candidate Release
1. Merge `develop` branch into `rc`
2. Bump package versions (e.g., v1.1.0-rc.1)
3. Update change log
4. Create release on GitHub from `rc` branch with the `rc` pre-release (e.g., `v1.1.0-rc.1`).
5. Build artifacts
6. Push artifacts

### Hotfix Release
1. Bump package versions (e.g., `v1.0.1-hotfix.1`)
2. Update change log
3. Create release on GitHub from `hotfix` branch with the `hotfix` pre-release (e.g., `v1.0.1-hotfix.1`).
4. Build artifacts
5. Push artifacts

### Alpha Release
1. Bump package versions with `alpha` pre-release segment and commit SHA (e.g., `v1.1.0-alpha.1+a30ddd1`)
2. Update change log
3. Create release on GitHub from `develop` branch with the `alpha` pre-release and commit SHA (e.g., `v1.1.0-alpha.1+a30ddd1`).
4. Build artifacts
5. Push artifacts

### Other pre-releases
Other pre-releases (e.g., `beta`, `pr`) should be released on demand.
They should follow the same process as alpha releases but with different pre-release segments (e.g., `-beta.1`, `-gamma.1`).

## Release Notes

Release notes should include:
- Version number and release date
- Highlights of the most important changes
- Upgrade instructions if needed
- Known issues or limitations
- Link to the full change log
- Contributor acknowledgment

## Release Schedule

We follow a predictable four-week release cycle, composed of two consecutive two-week sprints.
This schedule balances regular feature delivery with adequate testing time to ensure stability.

```
Week 1-2                  Week 3-4                  Week 5-6                  Week 7-8
┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐
│ Version N           │   │ Version N           │   │ Version N+1         │   │ Version N+1         │
│ Development Sprint  │   │ RC Testing Sprint   │   │ Development Sprint  │   │ RC Testing Sprint   │
│ & Alpha Testing     │   │                     │   │ & Alpha Testing     │   │                     │
└─────────────────────┘   └─────────────────────┘   └─────────────────────┘   └─────────────────────┘
       │                          │                        │                          │
       ▼                          ▼                        ▼                          ▼
┌─────────────┐            ┌─────────────┐          ┌───────────────┐            ┌─────────────┐
│ Alpha       │            │ RC Release  │          │ Alpha         │            │ RC Release  │
│ v(N)-alpha.1│            │ v(N)-rc.1   │          │ v(N+1)-alpha.1│            │ v(N+1)-rc.1 │
└─────────────┘            └─────────────┘          └───────────────┘            └─────────────┘
                                  │                                                    │
                                  │                                                    │
                                  ▼                                                    ▼
                            ┌─────────────┐                                      ┌─────────────┐
                            │ Stable      │                                      │ Stable      │
                            │ v(N)        │                                      │ v(N+1)      │
                            └─────────────┘                                      └─────────────┘
                                  ▲
                                  │
       ┌─────────────────────────┐│
       │ Hotfix (as needed)       ││
       │ from main branch        ││
       └─────────────────────────┘│
                                  │
                            ┌─────────────┐
                            │ Hotfix       │
                            │ v(N).0.1    │
                            └─────────────┘
```

### Development Cycle Details

1. **Development & Alpha Phase (Weeks 1-2)**
  - Active development of new features in the `develop` branch
  - Daily [alpha releases](#alpha-release) for internal testing
  - Developers conduct initial testing and validation

2. **Release Candidate Phase (Weeks 3-4)**
  - Code freeze at the end of Week 2
  - [Release a release candidate](#release-candidate-release)
  - Focus on stabilization, regression testing, and bug fixing
  - Only critical fixes merged to `rc` branch
  - New feature development for the next version begins in `develop` branch

3. **Stable Release (End of Week 4)**
  - Testing and release validation are finished
  - [Release stable version](#stable-release)

4. **Hotfix Process (As Needed)**
  - For critical production issues
  - Create a fix
  - [Release a hotfix](#hotfix-release)
  - Test and validate the issue
  - [Release stable version](#stable-release) with the hotfix included

This release cycle ensures a predictable cadence of stable releases while maintaining development velocity and providing a framework for addressing critical issues as they arise.

