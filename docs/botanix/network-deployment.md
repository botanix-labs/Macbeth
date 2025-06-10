# Network Deployment Strategy

## Networks

### Mainnet

Mainnet is our primary production environment that facilitates real transactions and interactions with the Bitcoin mainnet network.
It requires the highest level of security and stability.

- Permission-based network (only approved operators can run nodes)
- Future roadmap includes transition to a permissionless model
- Infrastructure: 16 federation nodes and 16 RPC nodes

### Testnet

Testnet serves as a testing environment for release candidates that closely mirrors mainnet functionality.
Business partners, developers, and community members use it to test applications and integrations before deploying to mainnet.

- Considered a production-grade network requiring stability and high availability
- Will support 3rd party testnet nodes in the future
- Infrastructure: 15 federation nodes and 15 RPC nodes

### Devnet

Development networks (devnets) provide environments for early testing, experimentation, and integration work.
These are internal networks that engineering teams can configure according to their specific testing requirements.

- Early feature testing and internal integration
- Temporary environments that can be created, reset, reconfigured, or destroyed as needed
- Support for experimental features that aren't ready for testnet


## Deployment Strategies

```
┌─────────────┐        ┌─────────────┐        ┌─────────────┐
│             │        │             │        │             │
│    DevNet   │        │   TestNet   │        │   MainNet   │
│             │        │             │        │             │
└──────┬──────┘        └──────┬──────┘        └──────┬──────┘
       │                      │                      │
       │                      │                      │
┌──────▼──────┐        ┌──────▼──────┐        ┌──────▼──────┐
│             │        │             │        │             │
│ Alpha       │        │  Release    │        │  Stable     │
│ Pre-Release ├───────►│  Candidate  ├───────►│  Release    │
│             │        │             │        │             │
└──────┬──────┘        └──────┬──────┘        └─────────────┘
       │                      │                      ▲
       │                      │                      │
       │                      │                      │
┌──────▼──────┐        ┌──────▼──────┐               │
│             │        │             │               │
│  Hotfix     │───────►│  Hotfix     │───────────────┘
│  DevNet     │        │  TestNet    │
│             │        │             │
└─────────────┘        └─────────────┘
```

### Alpha Pre-release Deployments

Alpha pre-releases are deployed to a dedicated alpha devnet for initial testing, following our established release schedule and testing strategy.

- Minimum infrastructure: 5 federation nodes and 1 RPC node
- Must include bridge and sidecar services
- Environment should support easy network reset and reconfiguration
- Intended for internal testing before broader release

### Release Candidate Deployments

Release candidates (RCs) are deployed to the testnet environment for final verification before stable release to mainnet.

- Should maintain testnet stability and functionality
- Requires robust contingency plans for quick recovery in case of issues
- Undergoes comprehensive testing with existing applications and services
- Serves as the final quality gate before mainnet deployment

### Hotfix Release Deployments

Hotfix releases address critical issues and may be deployed to either testnet or devnet depending on:
- The severity of the issue
- The potential impact on testnet stability
- The urgency of the fix
- Testing requirements before mainnet deployment

### Stable Release Deployments

Stable releases represent thoroughly tested software ready for production use on the mainnet.

- Deployment follows careful coordination with the community and partners
- Includes comprehensive release notes and migration instructions if applicable
- Follows a scheduled rollout plan with monitoring for any issues

### Other Pre-release Deployments

All pre-releases except release candidates are considered early development releases and should be deployed only to devnets.

- Teams can provision additional devnets for specific pre-release testing when needed
- These environments provide isolation for testing potentially disruptive changes
- Helps prevent negative impacts on more stable environments
