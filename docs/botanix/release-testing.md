# Release Testing Strategy

TODO: We should express that devs should lead testing of their PRs

## Test Types

### Smoke Tests

Smoke tests serve as the first line of defense in our testing strategy, designed to quickly verify that the system's core functionality operates as expected after deployment.

These tests confirm that:

- Required ports are listening and accessible
- RPC endpoints respond correctly to basic queries
- Block production occurs at the expected rate
- Basic network connectivity functions properly
- Essential services are operational

These lightweight tests identify obvious failures early before investing time in more comprehensive testing.
The specific implementation details should be detailed and proposed as an additional ADR.

### Automated Test Suite

Our comprehensive automated test suite provides systematic verification of system functionality and consists of two primary components:

**Functional Tests**
- Target specific features and isolated components
- Verify correctness of individual functions and modules
- Ensure proper handling of edge cases and error conditions
- Focus on unit-level and integration-level verification

**End-to-End (E2E) Tests**
- Simulate complete user workflows and scenarios
- Test the entire system as an integrated whole
- Verify proper interaction between all system components
- Validate behavior from an external user's perspective

The detailed implementation of these test suites should be described in a dedicated ADR to ensure consistent application across releases.

### Blockchain Compatibility Testing

Compatibility testing ensures that a new release maintains backward compatibility with existing blockchain data and can successfully interact with stable networks.

This testing involves:

1. Syncing a new node from genesis block to current height using the updated software
2. Verifying that all historical blocks are processed correctly
3. Ensuring compatibility with both testnet and mainnet blockchains

This testing is critical to prevent consensus failures or unexpected state divergence after deployment.

### Manual Testing

Despite extensive automation, certain aspects of the system benefit from human verification and exploratory testing.

Manual testing includes:

- Verification of newly delivered features against requirements
- Verification of the main functionality (pegin/pegout, etc.)
- Visual confirmation of proper operation
- Testing scenarios that are difficult to automate

A detailed manual testing protocol should be defined in a separate ADR to ensure consistency across release cycles.

## Testing Process

### Alpha Release Testing

Alpha releases undergo initial validation to identify major issues before broader deployment:

1. **Smoke Tests**: Verify basic functionality and system stability
2. **Automated Test Suite**: Run the complete test suite to identify regressions
3. **Blockchain Compatibility Testing**: Confirm compatibility with testnet and mainnet blockchains
4. **Manual Testing**: Verify new features and conduct exploratory testing

**Progression Criteria:**
- An alpha release that passes all tests can be promoted to release candidate status
- If significant issues are discovered, fixes must be implemented and the code either:
  - Reverted to a stable state
  - Moved to the next release cycle with appropriate fixes

### Release Candidate Testing

Release candidates undergo extended testing on the testnet for two weeks.

This testing includes:

1. **Smoke Tests**: Continuous verification throughout the testing period
2. **Automated Test Suite**: Regular execution to ensure consistent behavior
3. **Blockchain Compatibility Testing**: Extended testing against both testnet and mainnet chains
4. **Manual Testing**: Comprehensive verification of all features, with emphasis on stability

**Promotion to Stable Release:**
- A release candidate can be promoted to stable release only after:
  - All identified issues have been resolved
  - The software has operated without significant incidents for the full testing period
  - Stakeholders have approved the release
