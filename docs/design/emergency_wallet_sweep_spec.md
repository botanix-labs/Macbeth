**This file provides a comprehensive, auditable, and forward-compatible specification for the Botanix Emergency Wallet Sweep feature.**

# Emergency Wallet Sweep: Engineering Specification

## 1. Introduction & Guiding Principles

This document specifies the design and implementation of an emergency wallet sweep feature for the Botanix federation. This is a critical security and operational tool of last resort, designed to secure all funds in the federation's Bitcoin wallet by transferring them to a new, safe address.

### **Comprehensive Specification Scope**
This specification covers three distinct implementation phases:
1. **Emergency Sweep (Immediate)**: Single-key emergency operations using FROST infrastructure
2. **Post-Emergency Recovery (Immediate Post-Emergency)**: Multi-key generation support for graceful recovery after the first emergency sweep
3. **TEE Integration (Future)**: Adaptation for Trusted Execution Environment infrastructure

The design is governed by the following principles:

- **Zero Public Exposure**: The mechanism does not introduce any public-facing endpoints or on-chain events. All coordination communication is encrypted and authenticated.
- **Deliberate Operator Control**: The sweep is a manual procedure, initiated and controlled by human operators through a dedicated command-line tool.
- **Infrastructure Integration**: The solution integrates with `btc-server` components, including database, secure communication channels, and FROST threshold signing engine.
- **Multi-Key Compatibility**: The design supports multiple DKG key generations for post-emergency graceful recovery scenarios.
- **TEE Independence**: Multi-key support is orthogonal to future TEE integration, ensuring compatibility with both FROST and TEE signing.
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

To address the challenges of divergent UTXO sets and offline members, this feature uses a Coordinator-Peer model with explicit coordinator authority validation.

- **Coordinator Authority**: The coordinator must be the same member designated as the DKG coordinator for the active key generation. This authority is validated through cryptographic signatures using the coordinator's federation private key.
- **Secure Communication**: The coordinator directly queries member UTXO state via an authenticated gRPC endpoint using standard peer authentication mechanisms. Coordination messages are shared manually by operators through external communication channels (e.g., Discord, Slack, or other secure messaging platforms).
- **Coordinator-Led Consensus**: The Coordinator collects UTXO state from reachable members, computes a "safe subset" of UTXOs that have reached consensus, and generates an auditable report of any funds that cannot be included due to insufficient consensus.
- **Deterministic PSBT Generation**: The designated Coordinator generates a single, definitive Partially Signed Bitcoin Transaction (PSBT) using deterministic construction rules to ensure all members can independently verify and recreate the exact same transaction structure.
- **Threshold-Based Signing**: The FROST signing engine handles offline or lost members, proceeding as long as the minimum signature threshold (t) is met.

---

## 4. The Step-by-Step Emergency Workflow

### (Coordinator) Phase 1: Collect States and Initiate Sweep

- The designated Coordinator operator runs the `initiate` command with federation configuration:  
  `emergency-tool sweep initiate --destination <SECURE_ADDR> --fee-rate <RATE> --consensus-threshold 80 --federation-config <PATH> --coordinator-key <PATH>`
- This command performs several critical actions:
    1. **Validate Coordinator Authority**: Verifies that the operator holds the coordinator private key for the active key generation by requiring a cryptographic signature using the coordinator's federation private key.
    2. **Query Reachable Members**: Directly queries each federation member's `btc-server` via the `GetMemberUtxoState` gRPC endpoint to collect current UTXO state:
        - Reads the `FederationTomlConfig` from the specified path
        - Extracts member socket addresses from `federation_member_public_key[].socket_addr` fields
        - Establishes authenticated gRPC connections using JWT authentication
        - Sends `GetMemberUtxoState` requests with configurable timeout (default: 30 seconds per member)
        - Implements exponential backoff retry logic (3 attempts, 1s/2s/4s delays) for transient failures
        - Marks persistently unreachable members as offline after retry exhaustion
    3. **Compute Safe Subset**: Uses the collected UTXO lists to identify UTXOs recognized by at least the percentage of *reachable* members defined by `--consensus-threshold`. Note: UTXOs from offline members are not considered since their state is unknown.
    4. **Generate Audit Report**: Creates a detailed `excluded_utxos.json` report listing every UTXO that did not meet the consensus threshold and which specific members reported it.
    5. **Generate Deterministic PSBT**: Creates the definitive PSBT using deterministic construction rules to ensure all members can independently recreate the identical transaction.
    6. **Create Sweep Request**: Packages the PSBT, complete consensus data, and excluded UTXO details into a `SweepRequest` JSON file.
    7. **Distribute Request**: The coordinator distributes the `SweepRequest` file to all federation members via secure external channels.
    8. **Begin Signing**: The coordinator immediately starts the FROST signing process without waiting for explicit confirmations.

### (Peers) Phase 2: Accept Request and Join Signing

- Peer operators receive the `SweepRequest` JSON file from the coordinator via external communication channels
- **Automated Verification**: Each operator validates the request using:
  `emergency-tool sweep accept-request ./sweep-request.json`
- The tool performs comprehensive validation:
    1. **Coordinator Authority**: Verifies the coordinator signature against the known federation key
    2. **Consensus Verification**: Validates that consensus threshold was applied correctly to all UTXOs using the complete member reports and excluded UTXO data
    3. **PSBT Reconstruction**: Queries local `btc-server` database for all available UTXOs, applies the same consensus filtering using the provided parameters, and constructs PSBT using deterministic emergency PSBT builder with identical rules
    4. **Exact Validation**: Compares the locally reconstructed PSBT with the coordinator's PSBT using byte-for-byte comparison to ensure perfect consistency
    5. **Data Integrity**: Verifies the data integrity hash to ensure no tampering with consensus data or PSBT
    6. **Parameter Verification**: Validates destination address format, fee rate reasonableness, and consensus threshold application
- **Immediate Participation**: Upon successful validation and operator confirmation, the tool automatically joins the ongoing FROST signing session.

### (All) Phase 3: Unified Threshold Signing

- **Signing-Time Security**: All members who accepted the sweep request participate in FROST signing by providing the same request file:
  `emergency-tool sweep sign --request-file ./sweep-request.json`
- **Re-verification**: Before contributing signatures, each member's tool re-validates that the PSBT being signed exactly matches the PSBT in their accepted request file, preventing substitution attacks.
- **Natural Coordination**: The FROST protocol handles threshold coordination automatically. As soon as `t` signatures are collected, the transaction is finalized and broadcast.
- **Robust Completion**: If insufficient members participate, the signing naturally times out, allowing the process to be retried with adjusted parameters or member participation.

---

## 5. Post-Emergency Infrastructure Requirements

### **Multi-Key Generation Support (Post-Emergency Phase)**
After the first emergency sweep, the system must immediately support graceful recovery scenarios where multiple DKG key generations coexist:

- **Key Storage Enhancement**: The database stores multiple key packages with versioning (current + deprecated keys)
- **Key Selection Logic**: During signing, the system selects the correct key generation for each UTXO based on explicit UTXO-to-key-generation mappings
- **UTXO-Key Association**: UTXOs are explicitly mapped to their originating key generation during pegin processing
- **Coordinator Authority Control**: Only coordinators of the active key generation can initiate new DKG processes or emergency sweeps
- **Deprecated Key Maintenance**: Deprecated keys are preserved for potential operations on legacy UTXOs

### **Post-Emergency Multi-Key Considerations**
- **Cross-Generation UTXOs**: Post-emergency sweeps may include UTXOs from multiple key generations in a single transaction
- **Mixed-Key PSBTs**: PSBT construction and validation must handle inputs requiring different key generations
- **Key-Aware Signing**: The signing process routes each input to the appropriate key generation for signature creation
- **Transition-Safe Operations**: Emergency sweeps work correctly during and after DKG key transitions

### **Post-Emergency Database Schema Extensions**
The current implementation stores only a single key package. Post-emergency multi-key support requires:

```rust
// Current: Single key storage
const TREE_KEY_PACKAGE: &[u8; 5] = b"keypk";
const TREE_PUBKEY_PACKAGE: &[u8; 5] = b"pubpk";

// Required: Multi-key storage with versioning
const TREE_KEY_GENERATIONS: &[u8; 6] = b"keygens";  // {key_gen_id: u32} -> KeyGeneration
const TREE_CURRENT_KEY_GEN: &[u8; 6] = b"curgen";   // () -> current_key_gen_id: u32
const TREE_UTXO_KEY_MAP: &[u8; 6] = b"utxkey";      // {outpoint: OutPoint} -> key_gen_id: u32
const TREE_KEY_METADATA: &[u8; 6] = b"keymta";      // {key_gen_id: u32} -> KeyMetadata
```

### **Implementation Phases & TEE Dependency**

**Phase 1 - Emergency Sweep (Immediate)**: 
- Operates with single-key infrastructure using current `get_key_package()` and FROST signing
- No TEE dependency - uses existing FROST signing infrastructure

**Phase 2 - Post-Emergency Multi-Key Support (Immediate Post-Emergency)**:
- Multi-key generation support for graceful recovery after first emergency
- Requires database schema extensions for key versioning
- TEE Independence: Multi-key support works with both FROST and future TEE signing

**Phase 3 - TEE Integration (Future)**:
- Shifts signing from FROST to TEE infrastructure  
- Requires `p_L1` payload construction instead of PSBTs
- Both single-key and multi-key emergency sweeps must work with TEE

---

## 6. Detailed Implementation Specification

### Implementation Roadmap & File Structure

**New Components to Create:**
```
bin/emergency-tool/               # New emergency coordination CLI
├── Cargo.toml                   # Dependencies: tonic, tokio, bitcoin, btcserverlib, serde_json
├── src/
│   ├── main.rs                  # CLI argument parsing and command dispatch
│   ├── federation.rs            # Federation member discovery and coordinator validation
│   ├── utxo_consensus.rs        # UTXO consensus logic and audit reporting
│   ├── grpc_client.rs           # gRPC client management with retry logic
│   ├── message_io.rs            # Message file I/O for manual Discord/messaging sharing
│   ├── psbt_builder.rs          # Deterministic emergency PSBT construction
│   └── session.rs               # Emergency session management and validation
```

**Components to Modify:**
```
bin/btc-server/proto/btc_server.proto          # Add GetMemberUtxoState endpoint
bin/btc-server/src/bin/main.rs                 # Add emergency endpoint implementations
```

### Epic 1: `emergency-tool` — The Secure Coordination CLI

- **[ET-SESSION-01] Implement Emergency Session Management**
    - **Task**: Create session lifecycle management for emergency operations with isolation from normal operations.
    - **Session Structure**:
        ```rust
        struct EmergencySession {
            session_id: [u8; 32],           // Cryptographically random session identifier
            coordinator_id: frost::Identifier, // Federation member ID of coordinator
            coordinator_signature: Vec<u8>,    // Signature proving coordinator authority
            created_at: SystemTime,
            status: SessionStatus,
            participants: HashSet<frost::Identifier>,
        }
        
        enum SessionStatus {
            Initiated,     // Coordinator has started the session
            Collecting,    // Gathering UTXO state from members
            Consensus,     // Computing consensus and generating PSBT
            Signing,       // Members are participating in FROST signing
            Completed,     // Transaction has been finalized and broadcast
            Failed(String), // Session failed with error reason
        }
        ```
    - **Session Isolation**: Emergency sessions are completely isolated from normal pegout operations
    - **Concurrent Prevention**: Only one emergency session can be active at a time per federation
    - **Session Timeout**: Sessions auto-expire after 2 hours to prevent resource leaks

- **[ET-AUTHORITY-01] Implement Coordinator Authority Validation**
    - **Task**: Establish and validate coordinator authority for emergency operations.
    - **Authority Determination**: The coordinator must be the same member designated as the DKG coordinator for the active key generation, determined by the `config.coordinator` field and federation configuration.
    - **Authority Validation**:
        ```rust
        fn validate_coordinator_authority(
            emergency_context: &[u8],
            coordinator_signature: &[u8],
            coordinator_id: frost::Identifier,
            federation_config: &FederationTomlConfig,
        ) -> Result<(), AuthorityError> {
            // Verify coordinator_id matches the DKG coordinator for active key generation
            // Verify signature over emergency context using coordinator's federation private key
            // Ensure coordinator hasn't been revoked or changed
        }
        ```
    - **Non-Coordinator Protection**: Non-coordinators cannot initiate emergency sweeps and receive clear error messages when attempting to do so

- **[ET-CONSENSUS-01] Implement UTXO Consensus Logic**
    - **Task**: Implement the `compute_safe_subset` function that handles reachable member consensus.
    - **Input Processing**:
        - Accept a map of `member_id -> UtxoSet` for all successfully queried members
        - Accept a list of `offline_member_ids` for members that could not be contacted after retries
        - Calculate effective consensus threshold based on reachable members only
    - **Consensus Algorithm**:
        - For each unique UTXO reported by any reachable member, count how many members report it
        - Include UTXOs that are reported by at least `consensus_threshold`% of reachable members
        - Example: If 5/7 members are reachable and threshold is 80%, a UTXO needs 4/5 reachable members (80% of reachable)
    - **Output Generation**:
        - **Consensus UTXOs**: List of UTXOs that met the threshold for PSBT construction, including which members reported each UTXO and consensus percentage
        - **Excluded UTXOs**: Complete list of UTXOs that failed to meet consensus threshold, with detailed reporting by member and exclusion reasons
        - **Member Reports**: Full record of what each reachable member reported for complete transparency
        - **Consensus Parameters**: Standardized parameters for deterministic PSBT reconstruction:
            - UTXO ordering: Sort by (txid, vout) lexicographically  
            - Fee rate: Exact sat/vB rate specified by coordinator
            - Change calculation: Standardized algorithm with deterministic address derivation
        - **Audit Statistics**: Summary metrics including total excluded value and offline member impact

- **[ET-PSBT-01] Implement Deterministic Emergency PSBT Construction**
    - **Task**: Create emergency-specific PSBT builder that generates identical PSBTs across all members.
    - **Emergency PSBT Builder**:
        ```rust
        fn build_emergency_psbt(
            consensus_utxos: &[EmergencyUtxo],
            destination: &Address,
            fee_rate: FeeRate,
            change_script: &ScriptBuf,
        ) -> Result<Psbt, EmergencyPsbtError> {
            // Deterministic UTXO ordering by (txid, vout)
            let mut sorted_utxos = consensus_utxos.to_vec();
            sorted_utxos.sort_by_key(|u| (u.outpoint.txid, u.outpoint.vout));
            
            // Create inputs with deterministic ordering
            let inputs: Vec<InputDTO> = sorted_utxos.iter().map(|u| InputDTO {
                outpoint: u.outpoint,
                output: u.output.clone(),
                eth_address: u.eth_address,
                version: u.version,
            }).collect();
            
            // Single destination output (no pegout tracking needed)
            let total_input_value = inputs.iter().map(|i| i.output.value).sum::<Amount>();
            let absolute_fee = calculate_emergency_fee(&inputs, destination, fee_rate)?;
            let output_value = total_input_value.checked_sub(absolute_fee)
                .ok_or(EmergencyPsbtError::InsufficientFunds)?;
            
            let destination_output = TxOut {
                value: output_value,
                script_pubkey: destination.script_pubkey(),
            };
            
            // Create PSBT using emergency-specific construction
            create_emergency_psbt(inputs, vec![destination_output])
        }
        ```
    - **Deterministic Rules**:
        - UTXO ordering: Sort by (txid, vout) lexicographically
        - Fee calculation: Precise weight-based calculation with no randomization
        - Transaction structure: Version 2, locktime 0, sequence MAX for all inputs
        - No change outputs for simplicity (coordinator specifies fee rate to consume all funds)
    - **Cannot reuse** `btc-server`'s `create_psbt` function due to pegout tracking requirements

- **[ET-PSBT-COMPARE-01] Implement PSBT Exact Validation**
    - **Task**: Create logic to compare PSBTs for exact byte-for-byte equivalence to ensure perfect consistency.
    - **Validation Approach**:
        - **Deterministic Construction**: Both coordinator and peers use identical PSBT construction rules
        - **Byte-Level Comparison**: Direct comparison of serialized PSBT bytes for maximum security
        - **Implementation Consistency**: Ensures all members run identical emergency tool versions
    - **Implementation**:
        ```rust
        fn validate_psbt_exact_match(
            coordinator_psbt: &[u8],
            local_psbt: &[u8],
        ) -> Result<(), PsbtMismatchError> {
            if coordinator_psbt == local_psbt {
                Ok(())
            } else {
                Err(PsbtMismatchError::ByteMismatch {
                    coordinator_hash: sha256(coordinator_psbt),
                    local_hash: sha256(local_psbt),
                })
            }
        }
        ```
    - **Security Benefits**: Catches any difference including subtle tampering attempts, implementation bugs, or version mismatches

- **[ET-CONSENSUS-VALIDATE-01] Implement Coordinator Consensus Validation**
    - **Task**: Enable peers to independently verify the coordinator's consensus decisions using complete transparency data.
    - **Validation Logic**:
        ```rust
        fn validate_coordinator_consensus(
            sweep_request: &SweepRequest,
            local_utxos: &[Utxo],
        ) -> Result<(), ConsensusValidationError> {
            // Verify excluded UTXOs were correctly excluded
            for excluded_utxo in &sweep_request.excluded_utxos {
                let consensus_pct = (excluded_utxo.reported_by.len() * 100) / sweep_request.consensus_params.reachable_members.len();
                
                if consensus_pct >= sweep_request.consensus_params.threshold_percent {
                    return Err(ConsensusValidationError::IncorrectExclusion {
                        outpoint: excluded_utxo.outpoint,
                        actual_consensus: consensus_pct,
                        required_threshold: sweep_request.consensus_params.threshold_percent,
                    });
                }
            }
            
            // Verify consensus UTXOs actually met threshold
            for consensus_utxo in &sweep_request.consensus_utxos {
                if consensus_utxo.consensus_percentage < sweep_request.consensus_params.threshold_percent {
                    return Err(ConsensusValidationError::InsufficientConsensus {
                        outpoint: consensus_utxo.outpoint,
                        actual_consensus: consensus_utxo.consensus_percentage,
                        required_threshold: sweep_request.consensus_params.threshold_percent,
                    });
                }
            }
            
            // Verify member report integrity
            for member_report in &sweep_request.member_reports {
                verify_member_signature(&member_report)?;
            }
            
            Ok(())
        }
        ```
    - **Fraud Detection**: Catches coordinator manipulation of consensus thresholds or incorrect UTXO inclusion/exclusion
    - **Transparency**: Complete visibility into which members reported which UTXOs and why decisions were made

- **[ET-NETWORK-01] Implement Federation Member Discovery and Connection**
    - **Task**: Enable reliable discovery and connection to federation members with proper retry logic.
    - **Configuration Source**: 
        - Use `FederationTomlConfig::from_str()` to read federation configuration
        - Extract member information using `get_federation_pks_from_path()` method
        - Use existing JWT authentication patterns from btc-server
    - **Connection Management**:
        - Create gRPC clients using `btc_server_client::BtcServerClient`
        - Implement exponential backoff retry logic: 3 attempts with 1s/2s/4s delays
        - Connection timeout: 30 seconds per member (configurable)
        - Mark members as offline only after retry exhaustion
    - **Network Partition Handling**:
        - Continue with available members if they meet consensus threshold
        - Log detailed connection failure reasons for post-incident analysis
        - Fail the emergency sweep if insufficient members are reachable for consensus

- **[ET-COMMS-01] Implement Emergency Message Types**
    - **Task**: Define structured JSON messages for emergency coordination.
    - **SweepRequest Structure**:
        ```rust
        #[derive(Serialize, Deserialize)]
        struct SweepRequest {
            session_id: [u8; 32],
            coordinator_id: u16,
            coordinator_signature: Vec<u8>,
            psbt_bytes: Vec<u8>,
            destination_address: String,
            consensus_params: ConsensusParameters,
            member_reports: Vec<MemberUtxoReport>,
            consensus_utxos: Vec<ConsensusUtxoInfo>,
            excluded_utxos: Vec<ExcludedUtxoInfo>,
            consensus_stats: ConsensusStatistics,
            data_integrity_hash: [u8; 32],
            created_at: u64, // Unix timestamp
        }
        
        #[derive(Serialize, Deserialize)]
        struct ConsensusParameters {
            fee_rate_sat_vb: u64,
            utxo_ordering: String, // "lexicographic"
            threshold_percent: u8,
            reachable_members: Vec<u16>,
        }
        
        #[derive(Serialize, Deserialize)]
        struct MemberUtxoReport {
            member_id: u16,
            utxos: Vec<EmergencyUtxoInfo>,
            timestamp: u64,
            member_signature: Vec<u8>,
        }
        
        #[derive(Serialize, Deserialize)]
        struct ConsensusUtxoInfo {
            outpoint: OutPoint,
            value_sat: u64,
            eth_address: Option<String>,
            version: u32,
            reported_by: Vec<u16>,
            consensus_percentage: u8,
        }
        
        #[derive(Serialize, Deserialize)]
        struct ExcludedUtxoInfo {
            outpoint: OutPoint,
            value_sat: u64,
            eth_address: Option<String>,
            version: u32,
            reported_by: Vec<u16>,
            consensus_percentage: u8,
            exclusion_reason: String,
        }
        
        #[derive(Serialize, Deserialize)]
        struct ConsensusStatistics {
            total_members: u8,
            reachable_members: u8,
            offline_members: Vec<u16>,
            consensus_utxos_count: u32,
            excluded_utxos_count: u32,
            total_value_sat: u64,
            excluded_value_sat: u64,
        }
        ```
    - **Complete Transparency**: Include full UTXO data and member reports for comprehensive peer validation
    - **Message Integrity**: Data integrity hash covers PSBT, consensus UTXOs, and excluded UTXOs for tamper detection

- **[ET-CLI-01] Implement the `sweep` Subcommands**
    - `emergency-tool sweep initiate --destination <ADDR> --fee-rate <RATE> --consensus-threshold <PCT> --federation-config <PATH> --coordinator-key <PATH> [--jwt-secret <PATH>] [--timeout <SECS>]`: 
        - **Coordinator Authority**: Validates coordinator private key and generates authorization signature
        - **Member Querying**: Implements retry logic and graceful offline member handling
        - **Consensus & PSBT**: Runs deterministic consensus algorithm and PSBT generation
        - **Immediate Signing**: Begins FROST signing process immediately after creating sweep request
        - **Output**: Generates audit files and shareable JSON for manual distribution
    - `emergency-tool sweep accept-request ./sweep-request.json [--btc-server-addr <ADDR>] [--jwt-secret <PATH>]`: 
        - **Authority Validation**: Verifies coordinator signature and authorization
        - **Consensus Validation**: Independently verifies coordinator's consensus decisions using complete member reports and excluded UTXO data
        - **PSBT Reconstruction**: Uses identical deterministic construction logic
        - **Exact Validation**: Compares PSBTs using byte-for-byte comparison for perfect consistency
        - **Data Integrity Check**: Validates data integrity hash to ensure no tampering
        - **Automatic Participation**: Immediately joins ongoing FROST signing session upon validation
    - `emergency-tool sweep sign --request-file ./sweep-request.json [--btc-server-addr <ADDR>] [--jwt-secret <PATH>]`: 
        - **File Re-verification**: Confirms PSBT being signed matches the accepted request file
        - **FROST Integration**: Participates in standard threshold signing with signing-time validation

---

### Epic 2: `btc-server` — Emergency Support Infrastructure

- **[BS-UTXO-01] Implement `GetMemberUtxoState` Endpoint**
    - **Task**: Create emergency-specific gRPC endpoint for authenticated UTXO state collection.
    - **Endpoint Addition**: Add to `bin/btc-server/proto/btc_server.proto`:
        ```protobuf
        rpc GetMemberUtxoState(GetUtxoStateRequest) returns (GetUtxoStateResponse);
        
        message GetUtxoStateRequest {
            string session_id = 1;                    // Emergency session identifier
            bytes coordinator_signature = 2;         // Coordinator authority proof
            uint64 timestamp = 3;                     // Request timestamp for basic replay protection
        }
        
        message GetUtxoStateResponse {
            uint32 member_id = 1;                    // This member's federation ID
            repeated EmergencyUtxoInfo utxos = 2;    // Complete UTXO set
            uint64 timestamp = 3;                    // State capture timestamp
            bytes member_signature = 4;              // Response integrity signature
        }
        
        message EmergencyUtxoInfo {
            bytes txid = 1;              // Transaction ID
            uint32 vout = 2;             // Output index  
            uint64 value = 3;            // Satoshi amount
            bytes script_pubkey = 4;     // Script public key
            string eth_address = 5;      // Associated Ethereum address (hex, empty if none)
            uint32 version = 6;          // UTXO version for compatibility
            bytes witness_utxo = 7;      // Serialized TxOut for PSBT witness_utxo field
            uint32 key_generation_id = 8; // Key generation ID (for Phase 2 multi-key support)
        }
        ```
    - **Implementation Pattern**:
        ```rust
        async fn get_member_utxo_state(
            &self, 
            req: Request<GetUtxoStateRequest>
        ) -> Result<Response<GetUtxoStateResponse>, Status> {
            // Validate JWT authentication
            self.validate_jwt(&req)?;
            
            // Validate coordinator authority and timestamp
            validate_emergency_session_auth(&req.get_ref())?;
            
            // Get UTXOs using existing database access
            let db_utxos = self.db.get_all_utxos().to_status()?;
            
            // Transform to emergency format with key generation metadata
            let emergency_utxos = db_utxos.into_iter().map(|utxo| {
                EmergencyUtxoInfo {
                    txid: utxo.outpoint.txid.to_byte_array().to_vec(),
                    vout: utxo.outpoint.vout,
                    value: utxo.output.value.to_sat(),
                    script_pubkey: utxo.output.script_pubkey.to_bytes(),
                    eth_address: utxo.eth_address.map(hex::encode).unwrap_or_default(),
                    version: utxo.version,
                    witness_utxo: bitcoin::consensus::encode::serialize(&utxo.output),
                    key_generation_id: self.get_utxo_key_generation(&utxo.outpoint)
                        .unwrap_or_else(|| self.get_current_key_generation_id().unwrap_or(0)),
                }
            }).collect();
            
            Ok(Response::new(GetUtxoStateResponse {
                member_id: self.config.identifier as u32,
                utxos: emergency_utxos,
                timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                member_signature: self.sign_response(&emergency_utxos)?,
            }))
        }
        ```
    - **Security**: Full JWT validation, coordinator authority verification, and response signing

- **[BS-SESSION-01] Implement Emergency Session Authorization**
    - **Task**: Create session management to isolate emergency operations from normal pegout processing.
    - **Session Storage**: In-memory session registry to track active emergency sessions
    - **Authorization Endpoint**:
        ```rust
        async fn authorize_emergency_session(
            &self,
            req: Request<AuthorizeSessionRequest>
        ) -> Result<Response<AuthorizeSessionResponse>, Status> {
            self.validate_jwt(&req)?;
            
            let request = req.into_inner();
            
            // Validate session exists and member has accepted sweep request
            let session = self.emergency_sessions.get(&request.session_id)
                .ok_or_else(|| badarg!("Session not found"))?;
            
            // Validate PSBT matches accepted sweep request
            let psbt = Psbt::deserialize(&request.psbt)?;
            if psbt.txid().to_byte_array() != session.accepted_psbt_hash {
                return Err(badarg!("PSBT hash mismatch"));
            }
            
            // Authorize this session for signing
            self.emergency_sessions.insert(request.session_id, session);
            
            Ok(Response::new(AuthorizeSessionResponse { authorized: true }))
        }
        ```
    - **Integration**: Emergency PSBTs can only be signed after proper session authorization

---

### Epic 3: Post-Emergency Multi-Key Infrastructure

- **[MK-DB-01] Multi-Key Database Schema Implementation**
    - **Database Schema Enhancement**: Add new sled trees to support multiple key generations:
        ```rust
        #[derive(Clone)]
        pub struct Db {
            // ... existing fields ...
            
            /// Multi-key generation storage
            key_generations: sled::Tree,           // {key_gen_id: u32} -> KeyGeneration
            current_key_gen: sled::Tree,           // () -> current_key_gen_id: u32  
            utxo_key_map: sled::Tree,             // {outpoint: OutPoint} -> key_gen_id: u32
            key_metadata: sled::Tree,             // {key_gen_id: u32} -> KeyMetadata
        }
        ```
    
    - **Data Structures**:
        ```rust
        #[derive(Debug, Serialize, Deserialize, Clone)]
        pub struct KeyGeneration {
            pub id: u32,
            pub key_package: frost::keys::KeyPackage,
            pub pubkey_package: frost::keys::PublicKeyPackage,
            pub created_at: SystemTime,
            pub status: KeyGenerationStatus,
        }
        
        #[derive(Debug, Serialize, Deserialize, Clone)]
        pub enum KeyGenerationStatus {
            Active,      // Current signing key
            Deprecated,  // Post-emergency, kept for legacy UTXOs
            Archived,    // No longer needed, can be pruned
        }
        
        #[derive(Debug, Serialize, Deserialize, Clone)]
        pub struct KeyMetadata {
            pub coordinator_id: frost::Identifier,
            pub federation_config_hash: [u8; 32],
            pub min_signers: u16,
            pub max_signers: u16,
            pub emergency_sweeps_count: u32,
        }
        ```
    
    - **Database Methods**:
        ```rust
        impl Db {
            /// Migrate from single-key to multi-key schema on first startup
            pub fn migrate_to_multi_key(&self, config: &Config) -> Result<(), Error> {
                // Check if already migrated
                if self.get_current_key_generation()?.is_some() {
                    return Ok(());
                }
                
                // Migrate existing key to generation 0
                if let Some(key_package) = self.get_key_package()? {
                    if let Some(pubkey_package) = self.get_public_key_package()? {
                        let key_gen = KeyGeneration {
                            id: 0,
                            key_package,
                            pubkey_package,
                            created_at: SystemTime::now(),
                            status: KeyGenerationStatus::Active,
                        };
                        
                        // Use actual configuration values, not hardcoded defaults
                        self.add_key_generation(key_gen, config.min_signers, config.max_signers)?;
                        self.set_current_key_generation(0)?;
                        
                        // Map all existing UTXOs to key generation 0
                        let utxos = self.get_all_utxos()?;
                        for utxo in utxos {
                            self.set_utxo_key_generation(&utxo.outpoint, 0)?;
                        }
                    }
                }
                
                Ok(())
            }
            
            /// Gets the key generation for a specific UTXO with proper fallback logic
            pub fn get_utxo_key_generation_safe(&self, outpoint: &OutPoint) -> Result<u32, Error> {
                // First check explicit mapping
                if let Some(key_gen_id) = self.get_utxo_key_generation(outpoint)? {
                    return Ok(key_gen_id);
                }
                
                // For unmapped UTXOs, determine generation based on UTXO age/block height
                // rather than defaulting to current generation (which could be wrong)
                match self.estimate_utxo_key_generation(outpoint)? {
                    Some(estimated_gen) => Ok(estimated_gen),
                    None => {
                        // Final fallback to generation 0 for legacy compatibility
                        warn!("No key generation mapping found for UTXO {}, defaulting to generation 0", outpoint);
                        Ok(0)
                    }
                }
            }
            
            /// Estimate key generation based on UTXO characteristics
            fn estimate_utxo_key_generation(&self, outpoint: &OutPoint) -> Result<Option<u32>, Error> {
                // Implementation would check UTXO creation time, block height, etc.
                // to determine which key generation was likely active when it was created
                // This prevents emergency sweeps from using wrong keys for unmapped UTXOs
                Ok(None) // Placeholder - requires integration with blockchain data
            }
        }
        ```
    
    - **Emergency Tool Multi-Key Integration**:
        ```rust
        /// Enhanced UTXO consensus that properly handles multi-key scenarios
        pub fn compute_multi_key_safe_subset(
            member_utxos: Vec<(u16, Vec<EmergencyUtxoWithKey>)>,
            consensus_threshold: u8,
        ) -> (Vec<EmergencyUtxoWithKey>, Vec<OutPoint>) {
            // Process UTXOs by key generation to ensure proper key selection
            let mut key_gen_groups: BTreeMap<u32, Vec<(u16, Vec<EmergencyUtxoWithKey>)>> = BTreeMap::new();
            
            // Group UTXOs by their key generation
            for (member_id, utxos) in member_utxos {
                for utxo in utxos {
                    key_gen_groups.entry(utxo.key_generation_id)
                        .or_insert_with(Vec::new)
                        .push((member_id, vec![utxo]));
                }
            }
            
            // Run consensus algorithm separately for each key generation
            let mut all_consensus_utxos = Vec::new();
            let mut all_excluded_inputs = Vec::new();
            
            for (key_gen_id, key_gen_utxos) in key_gen_groups {
                let (consensus_utxos, excluded_inputs) = compute_safe_subset(key_gen_utxos, consensus_threshold);
                all_consensus_utxos.extend(consensus_utxos);
                all_excluded_inputs.extend(excluded_inputs);
                
                info!("Key generation {}: {} consensus UTXOs, {} excluded", 
                      key_gen_id, consensus_utxos.len(), excluded_inputs.len());
            }
            
            (all_consensus_utxos, all_excluded_inputs)
        }
        ```

---

## 7. Security & Operational Considerations

### **Security Measures**
- **Coordinator Authority**: Cryptographic validation prevents unauthorized emergency initiation
- **Session Isolation**: Emergency operations are completely isolated from normal pegout processing
- **Message Integrity**: All coordination messages include cryptographic signatures and checksums
- **Signing-Time Verification**: Re-validation of PSBTs at signing time prevents substitution attacks
- **Exact PSBT Matching**: Byte-for-byte comparison ensures perfect consistency across all members
- **Audit Trail**: Comprehensive logging of all consensus decisions and UTXO exclusions

### **Operational Guidelines**
- **Network Partition Tolerance**: System continues with available members if consensus threshold is met
- **Human Coordination**: Clear communication protocols for Discord/messaging coordination
- **Recovery Procedures**: Documented procedures for failed emergency sweeps or corrupted messages
- **Regular Testing**: Emergency sweep drills to validate operational readiness
- **Monitoring Integration**: Emergency session metrics integrated with existing telemetry

### **Error Handling & Recovery**
- **Connection Failures**: Exponential backoff retry with clear offline member identification
- **Consensus Failures**: Detailed reporting when insufficient members are available
- **PSBT Mismatches**: Comprehensive logging to diagnose synchronization issues
- **Session Timeouts**: Automatic cleanup of expired emergency sessions
- **Signing Failures**: Graceful handling of FROST signing errors with clear operator guidance

---

## 8. Testing & Validation Strategy

### **Unit Testing**
- **Consensus Algorithm**: Test with various member availability scenarios
- **PSBT Construction**: Validate deterministic construction across different environments
- **Authority Validation**: Test coordinator authority enforcement
- **Session Management**: Verify proper session isolation and lifecycle management

### **Integration Testing**
- **Multi-Member Scenarios**: Test with federation sizes from 3 to 7 members
- **Network Partition Simulation**: Test offline member handling and retry logic
- **FROST Integration**: Validate emergency PSBTs work with existing signing infrastructure
- **Multi-Key Operations**: Test post-emergency scenarios with multiple key generations

### **Operational Testing**
- **Manual Workflow**: Test complete Discord-based coordination workflow
- **Message Corruption**: Verify error handling for corrupted or tampered messages
- **Performance**: Validate acceptable performance with large UTXO sets
- **Recovery Scenarios**: Test failure recovery and operator error handling

---

## 9. Future TEE Integration

When TEE infrastructure is implemented, the emergency sweep feature will be adapted:

- **Payload Construction**: Replace PSBT generation with TEE `p_L1` payload construction
- **Authority Migration**: Move coordinator authority validation into TEE verification
- **Signing Integration**: Route emergency operations through TEE signing instead of FROST
- **Multi-Key Compatibility**: Ensure TEE integration maintains multi-key generation support

The current design's modular architecture ensures smooth migration to TEE-based signing while preserving all emergency coordination and consensus mechanisms.  