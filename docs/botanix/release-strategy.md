# Release Strategy

This document describes our release strategy: release branch-topology, branch management, versioning, and release schedule.

## Branch flow

```
      ┌──────────────────────────────┐
  ┌───│            main              │   v1.0.0, v1.1.0, etc.
  │   └──────────────────────────────┘
  │           ▲               ▲ 
  │           │               │ 
  |           │               ▼ 
  │           |        ┌─────────────┐
  │           |        │    hotfix   |   v1.0.0-hotfix.1+a30ddd1, v1.0.0-hotix.2+da23f3b
  │           |        └─────────────┘
  │           |               ▲        
  │           |               |                 
  │   ┌─────────────┐         |  
  │   │      rc     │         |          v1.1.0-rc.1, v1.1.0-rc.2
  │   └─────────────┘         |
  │           ▲               │
  │           │               │
  │           ▼               │
  │   ┌─────────────┐         |
  └──▶│   develop   │         |          v1.1.0-alpha.1+a30ddd1, v1.1.0-alpha.2+da23f3b
      └─────────────┘         |
              ▲               │
              │               │
              ▼               ▼
      ┌──────────────────────────────┐   
      │         a-new-change         |   A new PR branch
      └──────────────────────────────┘
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
- Adds `-hotfix.N` pre-release segment (e.g., `v1.0.1-hotfix.1+a30ddd1`)
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
- Releases with `-alpha.N+SHA` pre-release segment and build commit SHA (e.g., `v1.1.0-alpha.1+a30ddd1`)
- Version bumped based on changes:
    - New features: `MINOR` version bump
    - Other changes: `PATCH` version bump

## Release Process

### Stable Release

1. Merge `hotfix` or `rc` branches into `main` (fast-forward merge)
2. Bump package versions (e.g., `v1.1.0`)
3. Update change log
4. Back-merge changes to `develop` (fast-forward merge)
5. Force-merge `main` to `rc` and `hotfix`
6. Create release on GitHub from `main` branch with the version number (e.g., `v1.1.0`)
7. Build artifacts
8. Push artifacts

### Release Candidate Release
1. Merge `develop` branch into `rc` (fast-forward merge)
2. Bump package versions (e.g., v1.1.0-rc.1)
3. Update change log
4. Create release on GitHub from `rc` branch with the `rc` pre-release (e.g., `v1.1.0-rc.1`)
5. Build artifacts
6. Push artifacts

### Hotfix Release
1. Bump package versions (e.g., `v1.0.1-hotfix.1+a30ddd1`)
2. Update change log
3. Create release on GitHub from `hotfix` branch with the `hotfix` pre-release (e.g., `v1.0.1-hotfix.1+a30ddd1`)
4. Build artifacts
5. Push artifacts

### Alpha Release
1. Bump package versions with `alpha` pre-release segment and commit SHA (e.g., `v1.1.0-alpha.1+a30ddd1`)
2. Update change log
3. Create release on GitHub from `develop` branch with the `alpha` pre-release and commit SHA (e.g., `v1.1.0-alpha.1+a30ddd1`)
4. Build artifacts
5. Push artifacts

### Other pre-releases
Other pre-releases (e.g., `beta`, `pr`) should be released on demand.
They should follow the same process as alpha releases but with different pre-release segments (e.g., `-beta.1`, `-gamma.1`)

## Release Notes

Release notes should include:
- Version number and release date
- Highlights of the most important changes
- Upgrade instructions if needed
- Known issues or limitations
- Link to the full change log
- 3-rd party contributors acknowledgment

## Release Schedule

We follow a four-week release cycle with weeks 1-2 focused on development, week 3 dedicated to alpha testing and code freeze, and week 4 focused on RC testing.
This schedule balances rapid feature delivery with adequate testing time to ensure stability.

```
       Week 1-2                  Week 3                   Week 4
┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐
│ Version vX.Y.0      │   │ Version vX.Y.0      │   │ Version vX.Y.0      │
│ Development         │   │ Alpha Testing       │   │ RC Testing          │
│ & Initial Testing   │   │ & Code Freeze       │   │                     │
└─────────────────────┘   └─────────────────────┘   └─────────────────────┘
           │                         │                         │
           ▼                         ▼                         │
   ┌────────────────┐        ┌────────────────┐                │
   │ Alpha          │        │ Alpha          │                │
   │ vX.Y.0-alpha.1 │        │ vX.Y.0-alpha.Z │                │
   └────────────────┘        └────────────────┘                │
                                     │                         │
                                     ▼                         │
                             ┌────────────────┐                │
                             │  RC Release    │                │
                             │  vX.Y.0-rc.Z   │ ◄──────────────┘
                             └────────────────┘
                                     │
                                     │             ┌───────────────────────┐ 
                                     ▼             │ Next version vX.Y+1.0 │ Next cycle
  ┌───────────┐              ┌────────────────┐    │ Development           │───────────>
  │ Hotfix    │   if needed  │    Stable      │    └───────────────────────┘ 
  │ v(N).0.1  │◄─────────────┤    v(N).0.0    │
  └───────────┘              └────────────────┘
```

1. **Development (Weeks 1-2)**
- Focused on development of new features in the `develop` branch
- Regular [alpha releases](#alpha-release) for internal testing
- Developers conduct initial alpha testing and validation

2. **Alpha Testing & Code Freeze (Week 3)**
- Dedicated focus on alpha testing of all features
- Feature finalization and bug fixing
- Code freeze by the end of week 3
- [Release of first Release Candidate](#release-candidate-release) (`rc.1`) at the end of week 3

3. **Release Testing (Week 4)**
- Focused on comprehensive testing of release candidates on testnet
- Additional RC releases (`rc.N`) as needed based on testing results
- Only critical fixes merged to `rc` branch
- New version development (N+1) begins in `develop` branch as soon as the first RC is released

4. **Stable Release (End of Week 4)**
- Testing and release validation are finished
- [Release stable version](#stable-release)
- New version development (N+1) in `develop` branch

5. **Hotfix Process (As Needed)**
- For critical production issues
- Create a fix
- [Release a hotfix](#hotfix-release)
- Test and validate the issue
- [Release stable version](#stable-release) with the hotfix included

This release cycle ensures a predictable cadence of stable releases while maintaining development velocity and providing a framework for addressing critical issues as they arise. The ability to start new version development in the 4th week allows for continuous progress without waiting for the release to be finalized.

