**This file provides a comprehensive, auditable, and forward-compatible specification for the Botanix Emergency Wallet Sweep feature.**

# Emergency Wallet Sweep: Engineering Specification (v1.0)

## 1. Introduction & Guiding Principles

This document specifies the design and implementation of an emergency wallet sweep feature for the Botanix federation. This is a critical security and operational tool of last resort, designed to secure all funds in the federation's Bitcoin wallet by transferring them to a new, safe address.

The design is governed by the following non-negotiable principles:

- **Zero Public Exposure**: The mechanism will not introduce any new public-facing endpoints or on-chain events. All coordination communication will be encrypted and authenticated.
- **Deliberate Operator Control**: The sweep is a manual procedure, initiated and controlled by human operators through a dedicated command-line tool.
- **Leverage Proven Infrastructure**: The solution will be built upon the `btc-server`'s existing, battle-tested components, including its database, secure peer-to-peer communication channels, and FROST threshold signing engine.
- **Forward Compatibility**: The design is modular and anticipates future integration with a Trusted Execution Environment (TEE).

---

## 2. Core Scenarios & Threat Model

This feature is designed to address two primary emergency scenarios:

- **Scenario A: Active Key Compromise / Imminent Threat**  
  A situation where one or more of the federation's signing keys are believed to be compromised, requiring an immediate consolidation of funds to prevent theft.

- **Scenario B: Proactive Key Rotation due to Loss of Quorum**  
  A scenario where multiple federation members have gone permanently offline, risking the loss of signing quorum (`t` of `n`). A sweep allows the remaining members to move funds to a new wallet controlled by a reconstituted federation.

---

## 3. High-Level Architecture: Integrated Emergency Coordination

To address the challenges of divergent UTXO sets and offline members, this feature uses a Coordinator-Peer model facilitated by the existing DKG communication channel.

- **Secure Communication**: Operators will use new commands that send signed, encrypted messages through the existing `NewDkgPayload`/`GetDkgPayloads` gRPC endpoints. This provides an auditable, integrated, and robust channel for coordination.
- **Coordinator-Led Consensus**: To resolve inevitable state discrepancies between members, the Coordinator is responsible for collecting the state of all peers, computing a "safe subset" of UTXOs that have reached consensus, and generating an auditable report of any excluded funds.
- **Coordinator-Generated PSBT**: The designated Coordinator generates a single, definitive Partially Signed Bitcoin Transaction (PSBT) for the sweep based on the consensus UTXO set. This ensures all members sign the exact same transaction.
- **Threshold-Based Signing**: The existing FROST signing engine naturally handles offline or lost members, proceeding as long as the minimum signature threshold (t) is met.

---

## 4. The Step-by-Step Emergency Workflow

### (All Members) Phase 1: Report Local State

- Each federation member operator (including the coordinator) runs a command to broadcast their view of the wallet's state:  
  `emergency-tool sweep report-state`
- This command reads the local `btc-server` database to get its list of UTXOs, constructs a signed `ReportState` message, and sends it to the designated Coordinator via the `NewDkgPayload` gRPC endpoint.

### (Coordinator) Phase 2: Build Consensus and Initiate Sweep

- The designated Coordinator operator runs the `initiate` command with a parameter to define consensus:  
  `emergency-tool sweep initiate --destination <SECURE_ADDR> --fee-rate <RATE> --consensus-threshold 80`
- This command performs several critical actions:
    1. **Collect States**: It calls `GetDkgPayloads` to retrieve all `ReportState` messages from peers.
    2. **Compute Safe Subset**: It uses the collected UTXO lists as input to a `compute_safe_subset` function, which produces a final set of UTXOs recognized by at least the percentage of members defined by `--consensus-threshold`.
    3. **Generate Audit Report**: The function generates a detailed `excluded_utxos.json` report listing every UTXO that did not meet the threshold and which members reported it. This file is saved locally for auditing.
    4. **Generate Definitive PSBT**: It uses only the "safe subset" of consensus UTXOs to generate the single, definitive PSBT for the sweep.
    5. **Broadcast `StartEmergencySweep`**: It constructs and broadcasts a `StartEmergencySweep` message containing the definitive PSBT and a hash of the audit report.

### (Peers) Phase 3: Acknowledge and Validate

- Peer operators run a command to check for and process incoming coordination messages:  
  `emergency-tool sweep check-messages`
- Upon receiving a valid `StartEmergencySweep` message, each Peer's tool:
    1. Validates the Coordinator's signature.
    2. Validates that every UTXO used as an input in the definitive PSBT exists in its own local database. If the peer is missing a required input, it cannot sign and will raise a critical error.
    3. Sends an `AcknowledgeEmergencySweep` message back to the Coordinator via `NewDkgPayload`.

### (Coordinator) Phase 4: Distribute Definitive PSBT and Sign

- The Coordinator's `emergency-tool` collects the `AcknowledgeEmergencySweep` messages. Once a sufficient number of peers have acknowledged, it submits its definitive PSBT to its local `btc-server` to start the FROST signing process.

### (All) Phase 5: Unified Threshold Signing

- All acknowledged Peers fetch the definitive PSBT from the Coordinator and join the FROST signing session. This follows the existing, unmodified signing flow. As soon as `t` signatures are collected, the transaction is finalized and broadcast.

---

## 5. Detailed Implementation Specification

### Epic 1: `emergency-tool` — The Secure Coordination CLI

- **[ET-DB-01] Implement Database-as-a-Source**
    - **Task**: The tool must read UTXOs directly from a `btc-server` sled database.
    - **Interface**: Add a `--db-path <PATH>` argument.

- **[ET-CONSENSUS-01] Implement UTXO Consensus Logic**
    - **Task**: Implement the `compute_safe_subset` function as described in the recent commit. It must take a list of UTXO sets from members and a consensus threshold, and output one "safe" list of UTXOs and one `excluded_utxos.json` report.

- **[ET-PSBT-01] Implement High-Integrity PSBT Construction**
    - **Task**: The PSBT creation logic must correctly populate all required proprietary and witness fields (`witness_utxo`, `eth_address`, `version`) using the consensus UTXO list.

- **[ET-COMMS-01] Implement Emergency Message Types**
    - **Task**: Define new serializable structs for emergency coordination, to be wrapped in the `DkgPayload`.
    - **Structs**:
        - `ReportState { member_id: u16, utxos: Vec<Utxo>, signature: Vec<u8> }`
        - `StartEmergencySweep { session_id: [u8; 32], destination_address: String, fee_rate: u64, definitive_psbt: Vec<u8>, audit_report_hash: [u8; 32], signature: Vec<u8> }`
        - `AcknowledgeEmergencySweep { session_id: [u8; 32], member_id: u16, signature: Vec<u8> }`

- **[ET-CLI-01] Implement the `sweep` Subcommands**
    - `emergency-tool sweep report-state`: For all members. Reads local UTXOs and sends a `ReportState` message.
    - `emergency-tool sweep initiate ...`: For the Coordinator. Collects `ReportState` messages, runs consensus logic, generates PSBT, and broadcasts `StartEmergencySweep`.
    - `emergency-tool sweep check-messages`: For Peers. Calls `GetDkgPayloads`, validates messages, and sends `AcknowledgeEmergencySweep`.
    - `emergency-tool sweep sign ...`: For all acknowledged members. Authorizes the local `btc-server` to participate in the FROST signing.

---

### Epic 2: `btc-server` — Minor Adaptations for Emergency Payloads

- **[BS-PAYLOAD-01] Handle New Emergency Message Types**
    - **Task**: In the `new_dkg_payload` gRPC method, add logic to recognize and handle the `ReportState`, `StartEmergencySweep`, and `AcknowledgeEmergencySweep` message types.
    - **Logic**: Instead of processing them through the DKG state machine, simply store them in a new, separate database table/tree indexed by `session_id`. This makes them retrievable by the `emergency-tool`.

- **[BS-RPC-01] Implement an `AuthorizeEmergencySigning` Endpoint**
    - **Task**: Create a new, simple, authenticated gRPC endpoint.
    - **Endpoint**: `rpc AuthorizeEmergencySigning(AuthorizeRequest) returns (AuthorizeResponse);`
    - **Logic**: This endpoint takes a `session_id`. When called by an authenticated operator (via the `emergency-tool sweep sign` command), it flips a switch that "unlocks" the corresponding emergency PSBT, allowing it to be used in the standard `GetRound1/2SigningPackage` flow.

---

## 6. Future Work: Adaptation for TEE Integration

The implementation of the Pegout Proofs and TEE Environment will shift the locus of trust for signing from the FROST engine within `btc-server` to the isolated TEE. The emergency sweep feature must be adapted accordingly.

The `emergency-tool` will evolve from a signing *coordinator* to a TEE *payload constructor*. The `btc-server` will become a facilitator for the TEE.

The following tasks from the v1.0 specification would need to be re-implemented:

- **Re-implement [ET-PSBT-01] as [TEE-PAYLOAD-01]: Construct TEE `p_L1` Payload**
    - The tool's primary responsibility will no longer be to create a simple PSBT. It must construct the entire `p_L1` input parameter required by the TEE's `commit-l1` function. This involves gathering not just UTXOs, but also the required Botanix blocks, validator sets, and state proofs (`P_x`, `P_d`). The emergency sweep itself would need to be defined as a transaction type the TEE understands is valid without a corresponding user-initiated pegout.

- **Re-implement [ET-COMMS-01]: Evolve `StartEmergencySweep` Message**
    - The `StartEmergencySweep` message will need to be expanded to carry all the data necessary for every peer to deterministically reconstruct an identical `p_L1` payload to present to their local TEE. This is far more comprehensive than just a PSBT.

- **Re-implement [ET-CLI-01]: Reroute `sweep sign` Command**
    - The `emergency-tool sweep sign` command will no longer call an authorization endpoint on `btc-server`. Instead, it will submit the final, agreed-upon `p_L1` payload directly to the local TEE instance for validation and signing.

- **Replace [BS-RPC-01] with [TEE-RPC-01]: Implement `RequestTEESign` Endpoint**
    - The `AuthorizeEmergencySigning` RPC endpoint on `btc-server` will be deprecated. It will be replaced by a new method that passes the entire `p_L1` payload from the tool into the TEE. All authorization logic will be handled by the code programmed into the TEE itself.

- **New Task [TEE-FROST-01]: Solve Multi-Round Signing for Networkless TEE**
    - The TEE's "networkless" constraint poses a challenge for multi-round protocols like FROST. A new sub-protocol must be designed. This will likely involve the TEE consuming the `p_L1` payload, performing its validation and round-one calculations, and then outputting a signed `Round1Package`. The `emergency-tool` and `btc-server` would then be responsible for relaying these packages among peers and feeding them back into the TEE for the next round, ultimately collecting the partial signatures for final aggregation.

--- 