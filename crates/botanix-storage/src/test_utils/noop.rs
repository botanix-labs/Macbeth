use std::{
    collections::{HashMap, HashSet},
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};

use reth_chain_state::{
    CanonStateNotifications, CanonStateSubscriptions, ForkChoiceNotifications,
    ForkChoiceSubscriptions,
};
use reth_chainspec::{ChainInfo, ChainSpec, MAINNET};
use reth_db::models::{
    ChunkId, PeerID, Snapshot, SnapshotChunk, SnapshotId, SnapshotSync, SnapshotSyncId, UuidID,
    WalletStateSyncRecord,
};
use reth_db_api::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_errors::ProviderError;
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumber, BlockNumberOrTag,
    BlockWithSenders, Bytecode, Bytes, Header, Receipt, SealedBlock, SealedBlockWithSenders,
    SealedHeader, StorageKey, StorageValue, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber, Withdrawal, Withdrawals, B256, U256,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::StateProofProvider;
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdates, AccountProof, HashedPostState,
};
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use tokio::sync::{broadcast, watch};

use crate::{
    providers::StaticFileProvider,
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    ChainSpecProvider, ChangeSetReader, EvmEnvProvider, HeaderProvider, PruneCheckpointReader,
    ReceiptProviderIdExt, RequestsProvider, SnapshotReader, SnapshotWriter, StageCheckpointReader,
    StagedHeader, StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider,
    StaticFileProviderFactory, TransactionVariant, TransactionsProvider, WalletStateSyncReader,
    WalletStateSyncWriter, WithdrawalsProvider,
};

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone, Default, Copy)]
#[non_exhaustive]
pub struct BotanixNoopProvider;
