use crate::AuthorityConsensus;
use reth_primitives::ChainSpec;
use reth_provider::{BlockReaderIdExt, PostState, StateProvider};
use tokio::sync::{mpsc::UnboundedSender, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Builder type for confirguring the setup
pub struct AuthorityConsensusBuilder<Client, Pool> {
    client: Client,
    consensus: AuthorityConsensus,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage>,
    canon_state_notification: CanonStateNotificationSender,
    btc_server: BtcServerClient<tonic::transport::Channel>,
    secp: Secp256k1<All>,
    secret_key: secp256k1::SecretKey,
}

// ===== impl AuthorityConsensusBuilder =====
impl<Client, Pool> AuthorityConsensusBuilder<Client, Pool>
where
    Client: BlockReaderIdExt,
    Pool: TransactionPool,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        pool: Pool,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        mode: MiningMode,
        btc_server: BtcServerClient<tonic::transport::Channel>,
        secp: Secp256k1<All>,
        secret_key: secp256k1::SecretKey,
    ) -> Self {
        let latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());

        Self {
            storage: Storage::new(latest_header),
            client,
            consensus: AutoSealConsensus::new(chain_spec),
            pool,
            mode,
            to_engine,
            canon_state_notification,
            btc_server,
            secp,
            secret_key
        }
    }

    #[track_caller]
    pub fn build(self) -> (AuthorityConsensus, MiningTask<Client, Pool>) {
        let Self {
            btc_server,
            client,
            consensus,
            pool,
            mode,
            storage,
            to_engine,
            canon_state_notification,
            secp,
            secret_key
        } = self;

        //TODO: instantiate a new mining task

        (consensus)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    inner: Arc<RwLock<StorageInner>>,
}
