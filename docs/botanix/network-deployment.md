# Network Deployment Strategy

## Networks

### Mainnet

Mainnet is our primary production environment that facilitates real transactions and interactions with the Bitcoin mainnet network.
It requires the highest level of security and stability.

- Permission-based network (only approved operators can run nodes)
- Future roadmap includes transition to a permissionless model
- Infrastructure: 16 federation nodes and 2 RPC nodes. One of them is run by Botanix team. Bridge and sidecar services.
- Pegouts consensus threshold: 12 of 16 federation nodes
- Comet BFT consensus threshold: 11 of 16 federation nodes

### Testnet

Testnet serves as a testing environment that closely mirrors mainnet functionality.
Business partners, developers, and community members use it to test applications and integrations before deploying to mainnet.
Botanix teams use testnet to test release candidates and hotfixes before deploying to mainnet.

- Considered a production-grade network requiring stability and high availability
- Will support 3rd party testnet nodes in the future
- Infrastructure: 3 federation nodes and 2 RPC nodes. Bridge and sidecar services.
- Pegouts consensus threshold: 3 of 3 federation nodes
- Comet BFT consensus threshold: 3 of 3 federation nodes 

**Testnet infrastructure doesn't mirror mainnet's setup, which may lead to differences in behavior and hight risk of overseeing issues that could occur on mainnet**

### Devnet

Development networks (devnets) provide environments for early testing, experimentation, and integration work.
These are internal networks that engineering teams can sping up and configure according to their specific testing requirements.
Currently, we have one static devnet dedicated to alpha testing. In the future, when we see demand to test two long-running features in parallel, we'll allow developers to create their own devnets.

- Early feature testing and internal integration
- Temporary environments that can be created, reset, reconfigured, or destroyed as needed
- Support for experimental features that aren't ready for testnet
- Infrastructure: 6 federation nodes and 1 RPC node. Bridge and sidecar services.
- Pegouts consensus threshold: 4 of 6 federation nodes
- Comet BFT consensus threshold: 4 of 6 federation nodes

## Deployment Strategies


```
┌─────────────┐                   ┌─────────────┐        ┌─────────────┐
│             │                   │             │        │             │
│    DevNet   │                   │   TestNet   │        │   MainNet   │
│             │                   │             │        │             │
└─────────────┘                   └─────────────┘        └─────────────┘
       ▲                              ▲  ▲  ▲                   ▲
       │                      ┌───────┘  |  └──────────────┐    │
       │                      │          |                 |    │
┌─────────────┐        ┌─────────────┐                   ┌─────────────┐
│             │        │             │   |               │             │
│ Alpha       │        │  Release    │   |               │  Stable     │
│ Pre-Release ├───────►│  Candidate  ├──────────────────►│  Release    │
│             │        │             │   |               │             │
└─────────────┘        └─────────────┘   |               └─────────────┘
                                         |                      ▲
                                         │                      │
                                  ┌─────────────┐               │
                                  │             │               │
                                  │    Hotfix    │───────────────┘
                                  │             │
                                  └─────────────┘
```

### Alpha Pre-release Deployments

Alpha pre-releases are deployed to a dedicated alpha devnet for initial testing, following our established release schedule and testing strategy.

- Intended for internal testing before broader release
- We expect such releases to be unstable and may contain breaking changes so we should be ready to reset the devnet data if needed
- Testing and release acceptance criteria are defined in the [release-testing](./release-testing.md)

### Release Candidate Deployments

Release candidates (RCs) are deployed to the testnet environment for final verification before stable release to mainnet.

- Should maintain testnet stability and functionality
- Requires robust contingency plans for quick recovery in case of issues
- Serves as the final quality gate before mainnet deployment
- Testing and release acceptance criteria are defined in the [release-testing](./release-testing.md)

### Hotfix Release Deployments

Hotfix releases address critical issues and may be deployed to either testnet or devnet depending on:
- The severity of the issue
- The urgency of the fix

- Should maintain testnet stability and functionality
- Requires robust contingency plans for quick recovery in case of issues
- Serves as the final quality gate before mainnet deployment
- Testing and release acceptance criteria are defined in the [release-testing](./release-testing.md)

### Stable Release Deployments

Stable releases represent thoroughly tested software ready for production use on the mainnet.

- Deployment follows careful coordination with the community and partners
- Includes comprehensive release notes and migration instructions if applicable
- Follows a scheduled rollout plan with monitoring for any issues
- Testing and release acceptance criteria are defined in the [release-testing](./release-testing.md)

### Other Pre-release Deployments

All pre-releases except release candidates are considered early development releases and should be deployed only to devnets.

- Teams can provision additional devnets for specific pre-release testing when needed
- These environments provide isolation for testing potentially disruptive changes
- Helps prevent negative impacts on more stable environments
