# Release Testing Strategy

## Automated testing

### Test suite

We need an automated multistage testing tool that will be ran against any botanix network and treat it as a blackbox.
This will allow us to use the same tool with a single set of tests for local, CI, devnet, testnet and mainnet testing.
The implementation details will be proposed as a separate specification.

Test suite should consist of the following stages that could run all together or independently:
- Smoke Tests
- Functional Tests
- E2E Tests

### Smoke Tests

Smoke tests serve as the first line of defense in our testing strategy, designed to quickly verify that the system's core functionality operates as expected after deployment.

These tests confirm that:

- Required ports are listening and accessible
- RPC endpoints respond correctly to basic queries
- Block production occurs at the expected rate
- Basic network connectivity functions properly
- Essential services are operational

These lightweight tests identify obvious failures early before investing time in more comprehensive testing.

### Functional tests

Functional tests target specific features and isolated components (e.g., RPC endpoints)

- Target specific features and isolated components
- Verify correctness of individual functions and modules
- Ensure proper handling of edge cases and error conditions
- Focus on unit-level and integration-level verification

### E2E Tests

End-to-end (E2E) tests simulate complete user workflows and scenarios to validate the system's behavior from an external user's perspective (e.g pegin/pegout).

- Simulate complete user workflows and scenarios
- Test the entire system as an integrated whole
- Verify proper interaction between all system components
- Validate behavior from an external user's perspective

## Sidecar tests

Currently, while we don't have the test suite described above, we have to use our sidecar functional tests that cover some functionality and edge cases.

Sidecar functional tests: https://github.com/botanix-labs/Side-Car/tree/main/functional-tests

TODO: @scoottay instructions to run them.

## Blockchain Compatibility Testing

Compatibility testing ensures that a new release maintains backward compatibility with existing blockchain data.
It means new nodes will be able to sync blockchain from genesis up to tip without chain halt.
This testing is critical to prevent consensus failures or unexpected state divergence after deployment.

This testing involves:

1. Syncing a new node from genesis block to current height using the updated software
2. Verifying that all historical blocks are processed correctly up to the current tip
3. Ensuring compatibility with both testnet and mainnet blockchains. 
   Currently, testnet blockchain is already contains incompatible blocks, so we can test only against mainnet.
   To fix testnet blockchain, we need to reset testnet chain preserving the state. This process will be proposed in a separate specification.

## Manual Testing

Despite extensive automation, certain aspects of the system benefit from human verification and exploratory testing.

Manual testing includes:

- Verification of newly delivered features against requirements
- Verification of the main functionality (pegin/pegout, etc.)
- Visual confirmation of proper operation
- Testing scenarios that are difficult to automate

TODO: @scoottay provides specific testing scenarios.

## Testing Process

### Alpha Release Testing

Alpha releases undergo initial validation to identify major issues before broader deployment:

1. **Sidecar functional tests**: Run the complete test suite to identify regressions
2. **Manual Testing**: Verify new changes and standard testing scenarios.
   Developers assigned to the PR should lead testing of their changes.
3. **Blockchain Compatibility Testing**: Confirm compatibility with the mainnet blockchain.
   Should be run for mature alpha releases before we release RC.
4. Tested PRs must be marked as "verified on devnet/testnet" (columns on sprint board).

**Acceptance Criteria:**
- An alpha release that passes all tests can be promoted to release candidate status
- If significant issues are discovered:
  - Fixes must be made
  - An affected PR must be reverted and moved to the next release cycle

### Hotfix Release Testing

1. **Sidecar functional tests**: Run the complete test suite to identify regressions
2. **Manual Testing**: Verify new changes and standard testing scenarios.
   Developers assigned to the PR should lead testing of their changes.
3. **Blockchain Compatibility Testing**: Confirm compatibility with the mainnet blockchain.
   Can be skipped if at least two code owners confirm that consensus is not affected.
4. Tested PRs must be marked as "verified on devnet/testnet" (column on sprint board).

**Acceptance Criteria:**
- An alpha release that passes all tests can be promoted to stable release status
- If significant issues are discovered they must be fixed in the `hotfix` branch

### Release Candidate Testing

Release candidates undergo extended testing on the testnet for a week.

1. **Sidecar functional tests**: Run the complete test suite to identify regressions
2. **Manual Testing**: Verify new changes and standard testing scenarios.
   Developers assigned to the PR should lead testing of their changes.
3. **Blockchain Compatibility Testing**: Confirm compatibility with the mainnet blockchain.
   Can be skipped if at least two code owners confirm that consensus is not affected.
4. Tested PRs must be marked as "verified on devnet/testnet" (column on sprint board).

**Acceptance Criteria:**
- A release candidate can be promoted to stable release only after:
  - All tests have passed
  - The software has operated without significant incidents for the full testing period
  - Stakeholders have approved the release
- If significant issues are discovered:
  - Fixes must be made
  - An affected PR must be reverted and moved to the next release cycle
