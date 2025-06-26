# RFC: Improving Reth Upstream Integration and Customization Workflow

## Background

Our project, **Macbeth**, relies heavily on [reth-rs](https://github.com/paradigmxyz/reth), the modular and high-performance Ethereum execution layer written in Rust. 

Until now, our approach has been to **clone the official Reth repository**, apply our changes directly, and periodically pull in upstream changes. While this gives us full control, it has also made **upstream merges extremely painful**. 

Each new upstream release introduces a complex and error-prone merge process due to conflicts between our internal modifications and Reth's evolving codebase. As the pace of upstream development increases, this approach is becoming unsustainable.

## Problem Statement

Our current process creates several key challenges:

- **Manual and conflict-heavy upstream merges**: Every time we want to incorporate upstream updates from `paradigmxyz/reth`, we face merge conflicts that must be manually resolved due to the depth of our internal changes. Reth modules are often renamed, structurally interchanged or parts of them moved into other modules/crates.
- **Tight coupling**: Cloning and modifying the entire monorepo increases coupling between our fork and upstream.
- **Lack of granularity**: Updating means syncing the whole monorepo, even if we only need changes from a few specific crates.
- **Risk to long-term maintainability**: With upstream changing rapidly, we risk diverging too far from the mainline Reth, making future merges nearly impossible.

## Proposal: Modular Fork + Crate-Based Integration

To address these issues, we propose a **clean separation approach** that maintains upstream compatibility while enabling our customizations.
Our new approach consists of three main components:

1. **Clean Fork Management**: Maintain a minimal fork of reth that only adds extensibility improvements, not custom business logic
2. **Modular Integration**: Use individual reth crates from our fork via Cargo configuration, avoiding monorepo complexity
3. **Structured Project Organization**: Separate reth extensions from our business logic for better maintainability

**Key Principles**:
- **Extensibility First**: Fork changes only improve reth's modularity and extensibility
- **Upstream Friendly**: All changes designed to be proposable back to upstream
- **Clean Separation**: Business logic stays separate from reth integration code
- **Traceability**: Every change tracked and linked to specific PRs

This approach reduces merge conflicts, improves maintainability, and creates a sustainable path for long-term reth integration.

### Reth Fork Management

We maintain a fork at **https://github.com/botanix-labs/reth** with the following characteristics:

- **Minimal Changes**: Only patches that improve reth's extensibility
- **Upstream Friendly**: All changes designed to be proposable to upstream
- **Well Documented**: Every change tracked in `PATCHED_CRATES.md`
- **Tagged Releases**: Reth version with our patches traceability

#### Patched Crates Specification

We maintain a living document that tracks all modifications made to our Reth fork.

The `PATCHED_CRATES.md` file in the root of our fork repository:
```markdown
# Botanix Patched Crates

## Last Updated
- **Date**: 2025-06-20
- **Upstream Base**: v1.1.0 (commit: 66692a7)
- **Fork Tag**: botanix-reth+v1.1.0-patch.123

## Patched Crates

### reth-db-models
**Purpose**: Add extensibility for custom model types
**Changes**:
- Added `pub use` statements for internal macros (`impl_compression_for_compact!`)
- Made `ModelType` trait public and extensible
- Added `ExtensibleModel` trait for third-party implementations

**Files Modified**:
- `crates/storage/db-models/src/lib.rs`
- `crates/storage/db-models/src/models.rs`

**Upstream Readiness**: ✅ Ready - improves extensibility without breaking changes

### reth-chainspec
**Purpose**: Enable custom chain specification extensions
**Changes**:
- Made `ChainSpecBuilder` trait public
- Added `ExtensibleChainSpec` trait
- Exposed internal validation functions

**Files Modified**:
- `crates/chainspec/src/lib.rs`
- `crates/chainspec/src/spec.rs`

**Upstream Readiness**: ✅ Ready - enhances modularity

### reth-consensus
**Purpose**: Allow custom consensus implementations
**Changes**:
- Made `ConsensusEngine` trait more flexible
- Added optional trait methods with default implementations
- Exposed consensus validation utilities

**Files Modified**:
- `crates/consensus/src/lib.rs`
- `crates/consensus/src/engine.rs`

**Upstream Readiness**: 🔄 In Progress - needs documentation improvements
```

#### Git Tagging Strategy

Every update to our fork follows a tagging strategy that includes the PR number for traceability:

**Tag Format**: `botanix-reth-v{UPSTREAM_VERSION}+patch.{PR_NUMBER}`

Examples:
- `botanix-reth-v1.1.0+patch.123` - Patch based on reth v1.1.0, created in PR #123
- `botanix-reth-v1.1.0+patch.145` - Updated patch with fixes, created in PR #145
- `botanix-reth-v1.2.0+patch.146` - Updated reth version to v1.2.0, PR #146

Benefits of PR-based tagging:
- **Traceability**: Easy to find the exact PR that created the tag
- **Review History**: Can review the discussion and changes that led to the tag
- **Collaboration**: Multiple team members can see the context of changes
- **Debugging**: If issues arise, can quickly trace back to the originating PR

#### Apply New Patch

When applying patches to our fork, we follow this structured process:

1. **Plan the Extensibility Change**
    - Discuss with the Reth maintainers if they accept the change to upstream
    - Document the intended change and last update info in `PATCHED_CRATES.md`
    - Ensure the change only improves extensibility, doesn't add business logic
    - Verify the change would be acceptable upstream

2. **Implement the Patch**
    - Create a feature branch
    - Make minimal changes focused on exposing APIs or adding traits
    - Write comprehensive tests for new extensibility features, to make sure the changes don't break existing functionality
    - Mark upstream readiness status in `PATCHED_CRATES.md`
    - Add documentation for new public APIs

3. **Review and Merge**
    - Create PR with detailed description of extensibility improvements
    - Link to the relevant section in `PATCHED_CRATES.md`
    - Review for backwards compatibility and upstream acceptability
    - Merge with **squashing** after approval

4. **Tag and Release**
    - Create a new tag following our tagging strategy

**This specification must be updated as part of every patch process** and serves as our source of truth for what has been modified and why.

#### Upgrade Fork from Upstream

To keep our fork updated and in sync with the official Reth repository:

1. **Select an Upstream Release**  
    - Select a stable release or commit from the official Reth repository (e.g., `v1.1.0`)

2. **Create Update PR**  
    - Create a new branch and PR for the update branch

3. **Merge Upstream Changes**  
    - Merge the selected upstream commit into our update branch, resolving any conflicts
    - Make sure our extensibility patches are not overridden

4. **Test and Validate**  
    - Ensure all our extensions still work and that we haven't broken existing functionality

5. **Update Patched Crates Specification**  
    - Update `PATCHED_CRATES.md` to reflect the new upstream base and any changes made during the merge

6. **Review and Merge**
    - Review changes
    - Merge **without squashing** into the main branch

7. **Create New Tag**
   Follow the tagging strategy to create a new release tag.


#### Contributing Changes Back to Upstream Reth

Since all our fork changes are designed to improve reth's extensibility, we should actively contribute them back to the upstream project.

For each upstream-ready change, create a clean branch that excludes our internal documentation:

```bash
# Start from upstream main/master
git fetch upstream
git checkout upstream/main
git checkout -b enhance-db-models-extensibility

# Cherry-pick only the relevant commits (exclude PATCHED_CRATES.md changes)
git cherry-pick <commit-hash-for-db-models-changes>

# Remove any botanix-specific documentation or references
git reset HEAD~1  # Unstage the commit
# Edit files to remove internal references
git add .
git commit -m "enhance: make db-models more extensible for third-party implementations"
```

When a new branch is ready, create a PR against the upstream repository.

Once a change is accepted upstream, [update our fork](#upgrade-fork-from-upstream)

### Macbeth Integration

The key difference in our new integration approach is to use individual reth crates from our fork via Cargo configuration, avoiding copy-pastes and monorepo complexity.

#### Dependency Management

Instead of listing all reth dependency versions individually in root `Cargo.toml`, we can use `env` for centralized dependency management:

The `Cargo.toml` file in the root of our Macbeth repository:

```toml
[env]
BOTANIX_RETH_TAG = "botanix-reth-v1.1.0-patch.123"

[workspace.dependencies]
reth-primitives = { git = "https://github.com/botanix-labs/reth", tag = { env = "BOTANIX_RETH_TAG" } }
reth-stages = { git = "https://github.com/botanix-labs/reth", tag = { env = "BOTANIX_RETH_TAG" } }
```

This allows us to:
- Manage all reth dependency versions from a single configuration point
- Easily switch between different versions for testing

##### Updating Reth Dependencies

With our new the `env` approach, updating reth dependencies becomes simple.

Update the `BOTANIX_RETH_TAG`:

```toml
[env]
BOTANIX_RETH_TAG = "botanix-reth-v1.2.0-patch.145"  # Updated tag
```

Clear cargo cache and update:

```bash
# Clear Git cache
rm -rf ~/.cargo/git/checkouts
rm -rf ~/.cargo/git/db

# Update dependencies
cargo update

# Verify the correct version is being used
cargo tree | grep reth
```

##### Alternative: Use Environment Override

For testing different versions:

```bash
# Test with a specific tag
BOTANIX_RETH_TAG=botanix-reth-v1.1.0-patch.134 cargo build

# Or export for session
export BOTANIX_RETH_TAG=botanix-reth-v1.1.0-patch.134
cargo build
```

#### Extending Reth Fork Crates

##### Composition and Wrapping

For components that don't need internal access, we use composition in our `crates/reth/botanix-chain-spec` crate:

```rust
/// Botanix chain spec type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotanixChainSpec {
    /// [`ChainSpec`].
    pub inner: ChainSpec,

    /// The hash of the genesis block.
    pub genesis_hash: Option<B256>,

    /// The maximum gas limit
    pub max_gas_limit: u64,

    /// The number of confirmations we require for pegins from the mainchain.
    pub parent_confirmation_depth: u32,

    /// Block times in seconds
    pub leader_selection_window: Option<u64>,

    /// Botanix fee recipient
    pub botanix_fee_recipient: Option<String>,

    /// LST fee receiver
    pub lst_fee_receiver: Option<String>,
}

impl EthChainSpec for BotanixChainSpec {
    type Header = Header;
    // Implementation using inner ChainSpec
}

impl Hardforks for BotanixChainSpec {
    // Delegate to inner implementation with our customizations
}
```

##### Trait Implementation Integration

Integration with reth traits happens in the `crates/reth/` crates:

```rust
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BotanixPoolBuilder {
    domain_service: BotanixValidationService, // From crates/botanix/
}

impl<Types, Node> PoolBuilder<Node> for BotanixPoolBuilder
where
    Types: NodeTypes<ChainSpec = BotanixChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    // Implementation that bridges reth infrastructure with botanix business logic
}
```

## Implementation Plan

### Current State Assessment

Currently, we have a complex setup where:
- Reth crates are mixed with our custom business logic
- Changes are applied directly to a cloned reth repository
- Difficult to track what has been modified and why
- Hard to contribute changes back upstream

### Migration Strategy

To transition from our current setup to the proposed clean architecture:

#### Phase 1: Fork Preparation
1. **Create Clean Fork**
   ```bash
   # Fork the official reth repository to botanix-labs/reth
   # Clone the fork locally
   git clone https://github.com/botanix-labs/reth.git
   cd reth
   
   # Add upstream remote
   git remote add upstream https://github.com/paradigmxyz/reth.git
   ```

2. **Identify Current Commit Base**
   Ask Scott

3. **Create Initial Clean Fork**
   ```bash
   # In the new fork, checkout the same base commit
   git checkout <base-commit-hash>
   git checkout -b botanix
   ```
   Make `botanix` the default branch.

#### Phase 2: Define migration plan

- Review all modifications in macbeth repo
- Categorize changes into:
   - **Extensibility improvements** (can go to fork)
   - **Business logic** (should stay in macbeth)
   - **Mixed changes** (need to be split)
- Define tasks

#### Phase 3: Implement changes one by one

- For each extensibility improvement:
  - Create a new branch in the fork
  - Apply the change
  - Update `PATCHED_CRATES.md` with details
  - Create a PR against the fork
  - Fork's CI should be green
  - Review and merge the PR
  - Tag the fork
  - Use a new tag in macbeth's Cargo configuration and update code
  - Validate that macbeth passing CI

### Success Criteria

The implementation is successful when:
- [ ] Clean fork created with only extensibility improvements
- [ ] Macbeth repository uses the forked reth crates via Cargo configuration and doesn't contain copy-pasted code
- [ ] Continues Integration in fork and macbeth repositories are green

## Future Possible Improvements

## Separate Reth Extensions from Business Logic

Organize our codebase with a simple, clear separation between reth extensions and our own business logic:

```
macbeth/
├── crates/
│   ├── reth/                     # Reth integrations and extensions
│   │   ├── botanix-chain-spec/   # Wraps reth ChainSpec
│   │   ├── botanix-consensus/    # Consensus engine integration
│   │   ├── botanix-network/      # Network layer integration
│   │   ├── botanix-pool/         # Pool builder integration
│   │   ├── botanix-node/         # Node builder integration
│   │   └── botanix-db-models/    # Database model extensions
│   │
│   └── botanix/                  # Our own crates and business logic
│       ├── btc-wallet/           # Bitcoin wallet integration
│       ├── comet-bft-rpc/        # CometBFT RPC client
│       ├── data-parser/          # Data parsing utilities
│       └── frost/                # FROST signature scheme
│
└── .cargo/
    └── config.toml              # Centralized dependency configuration
```

**Benefits of this structure**:
- **Clear separation**: `crates/reth/` contains only reth extending logic, `crates/botanix/` contains our logic
- **Simple organization**: Easy to understand what depends on reth vs. what doesn't
- **Better maintainability**: Changes to reth integration are isolated to one directory
- **Upstream contribution**: Reth extensions can be easily extracted from `crates/reth/`

## Conclusion

This improved workflow ensures maintainable, upstream-friendly, and well-organized integration with reth while actively contributing back to the community and reducing our long-term maintenance burden. The clear separation of concerns, structured approach to patches, and comprehensive implementation plan provide a roadmap for transitioning from our current complex setup to a sustainable, long-term solution.
