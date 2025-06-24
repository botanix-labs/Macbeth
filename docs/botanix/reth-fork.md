# RFC: Improving Reth Upstream Integration and Customization Workflow

## Background

Our project, **Macbeth**, relies heavily on [reth-rs](https://github.com/paradigmxyz/reth), the modular and high-performance Ethereum execution layer written in Rust. 

Until now, our approach has been to **clone the official Reth repository**, apply our changes directly, and periodically pull in upstream changes. While this gives us full control, it has also made **upstream merges extremely painful**. 

Each new upstream release introduces a complex and error-prone merge process due to conflicts between our internal modifications and Reth's evolving codebase. As the pace of upstream development increases, this approach is becoming unsustainable.

## Problem Statement

Our current process creates several key challenges:

- **Manual and conflict-heavy upstream merges**: Every time we want to incorporate upstream updates from `paradigmxyz/reth`, we face merge conflicts that must be manually resolved due to the depth of our internal changes. Reth modules are often renamed, structurally interchanged or parts of them moved into other modues/crates.
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
- **Fork Tag**: botanix-reth-v1.1.0-patch.123

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

The key differences in our new integration approach:
1. Use individual reth crates from our fork via Cargo configuration, avoiding monorepo complexity
2. Separate reth extensions/integrations from our business logic for better maintainability

#### Dependency Management

Instead of listing all reth dependencies individually in `Cargo.toml`, we use Cargo's configuration system for centralized dependency management:

The `.cargo/config.toml` file in the root of our Macbeth repository:

```toml
[env]
BOTANIX_RETH_TAG = "botanix-reth-v1.1.0-patch.123"

[source.crates-io]
replace-with = "botanix-reth"

[source.botanix-reth]
git = "https://github.com/botanix-labs/reth"
tag = { env = "BOTANIX_RETH_TAG" }

# Override specific reth crates to use our fork
[source."https://github.com/paradigmxyz/reth"]
replace-with = "botanix-reth"
```

This allows us to:
- Manage all reth dependencies from a single configuration point
- Use our fork tags instead of commit hashes
- Easily switch between different versions for testing
- Override the source globally without modifying individual `Cargo.toml` files

##### Updating Reth Dependencies

With our new `.cargo/config.toml` approach, updating reth dependencies becomes simple.

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

#### Project Structure

We organize our codebase with a simple, clear separation between reth integrations and our own business logic:

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
- **Clear separation**: `crates/reth/` contains only reth integrations, `crates/botanix/` contains our logic
- **Simple organization**: Easy to understand what depends on reth vs. what doesn't
- **Better maintainability**: Changes to reth integration are isolated to one directory
- **Upstream contribution**: Reth extensions can be easily extracted from `crates/reth/`

#### Extending Reth Crates

##### Extending Crates via Public APIs

When our fork exposes extensibility points, we use them in our `crates/reth/` extensions:

**In our fork** (reth-db-models), we expose extensibility:
```rust
// Make the compression macro public for external use
pub use crate::compression::impl_compression_for_compact;

// Add extensibility trait
pub trait ExtensibleModel: Send + Sync {
    fn model_type(&self) -> u8;
    fn encode(&self) -> Vec<u8>;
    fn decode(data: &[u8]) -> Result<Self, DecodeError> where Self: Sized;
}
```

**In our crates/reth/ crate** (botanix-db-models):
```rust
use reth_db_models::{ExtensibleModel, impl_compression_for_compact};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotanixSpecificModel {
    // Our custom fields
}

impl ExtensibleModel for BotanixSpecificModel {
    // Implementation
}

// Use the now-public macro
impl_compression_for_compact!(BotanixSpecificModel);
```

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

##### Pure Business Logic Crates

Pure business logic lives in the `crates/botanix/` directory and has no direct dependency on reth.

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
- No clear separation between extensibility improvements and business logic
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
   ```bash
   # In your current reth clone, find the base commit
   git log --oneline | tail -20
   # Identify the upstream commit you're currently based on
   ```

3. **Create Initial Clean Fork**
   ```bash
   # In the new fork, checkout the same base commit
   git checkout <base-commit-hash>
   git checkout -b botanix-extensions
   ```

#### Phase 2: Extract Extensibility Patches
1. **Analyze Current Changes**
   - Review all modifications in your current reth clone
   - Categorize changes into:
     - **Extensibility improvements** (can go to fork)
     - **Business logic** (should move to macbeth)
     - **Mixed changes** (need to be split)

2. **Create `PATCHED_CRATES.md`**
   ```bash
   # In your fork
   touch PATCHED_CRATES.md
   # Document all planned extensibility changes
   ```

3. **Apply Extensibility Patches**
   ```bash
   # For each extensibility improvement:
   git checkout -b enhance-{crate}-extensibility
   # Apply only the extensibility parts
   # Create PR and merge
   # Tag the result
   git tag -a botanix-reth-v{VERSION}-patch.{PR_NUMBER}
   ```

#### Phase 3: Restructure Macbeth
1. **Create Directory Structure**
   ```bash
   # In macbeth repository
   mkdir -p crates/reth
   mkdir -p crates/botanix
   ```

2. **Move Business Logic**
   - Move pure business logic crates to `crates/botanix/`
   - Create wrapper/extension crates in `crates/reth/`
   - Update imports and dependencies

3. **Setup Cargo Configuration**
   ```bash
   # Create .cargo/config.toml
   cat > .cargo/config.toml << EOF
   [env]
   BOTANIX_RETH_TAG = "botanix-reth-v{VERSION}-patch.{PR_NUMBER}"
   
   [source.crates-io]
   replace-with = "botanix-reth"
   
   [source.botanix-reth]
   git = "https://github.com/botanix-labs/reth"
   tag = { env = "BOTANIX_RETH_TAG" }
   
   [source."https://github.com/paradigmxyz/reth"]
   replace-with = "botanix-reth"
   EOF
   ```

#### Phase 4: Testing and Validation
1. **Build and Test**
   ```bash
   # Clear cargo cache
   rm -rf ~/.cargo/git/checkouts
   rm -rf ~/.cargo/git/db
   
   # Build with new structure
   cargo build
   cargo test
   ```

2. **Functional Testing**
   - Ensure all functionality works with the new structure
   - Verify that reth extensions work correctly
   - Test that business logic is properly separated

3. **Documentation Update**
   - Update README files
   - Document the new architecture
   - Create migration guide for team members

#### Phase 5: Rollout
1. **Team Training**
   - Present the new architecture to the team
   - Provide hands-on training sessions
   - Create quick reference guides

2. **Gradual Migration**
   - Use feature flags to gradually move components
   - Maintain parallel builds during transition
   - Monitor for regressions

3. **Cleanup**
   - Remove old reth clone once migration is complete
   - Update CI/CD pipelines
   - Archive old workflows

### Success Criteria

The implementation is successful when:
- [ ] Clean fork created with only extensibility improvements
- [ ] All business logic moved to `crates/botanix/`
- [ ] All reth integrations moved to `crates/reth/`
- [ ] Cargo configuration manages all reth dependencies
- [ ] Full test suite passes
- [ ] Team can easily update reth dependencies
- [ ] Clear path for upstream contributions established
- [ ] Documentation updated and team trained

## Checklists

### Checklist for Reth Fork Updates

- [ ] **Planning Phase**
  - [ ] Select upstream reth version/commit to merge
  - [ ] Review upstream changes for potential conflicts
  - [ ] Check if any of our patches have been upstreamed
  - [ ] Plan testing strategy

- [ ] **Update Process**
  - [ ] Create update branch: `git checkout -b update-to-v{VERSION}`
  - [ ] Merge upstream changes: `git merge v{VERSION}`
  - [ ] Resolve any merge conflicts
  - [ ] Remove patches that have been upstreamed
  - [ ] Re-apply remaining extensibility patches
  - [ ] Update `PATCHED_CRATES.md` specification
  - [ ] Test all functionality works

- [ ] **Release Process**
  - [ ] Create annotated git tag following naming convention
  - [ ] Push tag to remote repository
  - [ ] Update `BOTANIX_RETH_TAG` in `.cargo/config.toml`
  - [ ] Test Macbeth builds with new version
  - [ ] Update documentation if needed

- [ ] **Verification**
  - [ ] Run full test suite
  - [ ] Verify all crates/reth/ extensions work correctly
  - [ ] Check that no functionality is broken
  - [ ] Confirm upstream readiness status for remaining patches

- [ ] **Upstream Contribution**
  - [ ] Identify new contribution-ready changes
  - [ ] Prepare clean PRs for upstream submission
  - [ ] Update contribution tracking documentation

## Conclusion

This improved workflow ensures maintainable, upstream-friendly, and well-organized integration with reth while actively contributing back to the community and reducing our long-term maintenance burden. The clear separation of concerns, structured approach to patches, and comprehensive implementation plan provide a roadmap for transitioning from our current complex setup to a sustainable, long-term solution.
