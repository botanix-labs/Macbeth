use crate::snapshots::core::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{self, Receiver, Sender},
};
pub struct Manager {
    extensions: HashMap<String, Box<dyn ExtensionSnapshotter>>,
    store: Arc<SnapshotStore>,
    opts: SnapshotOptions,
    multistore: Arc<dyn Snapshotter>,
    logger: Logger,

    mtx: Mutex<OperationState>,
}

impl Manager {
    pub fn new(
        store: Arc<SnapshotStore>,
        opts: SnapshotOptions,
        multistore: Arc<dyn Snapshotter>,
        extensions: HashMap<String, Box<dyn ExtensionSnapshotter>>,
        logger: Logger,
    ) -> Self {
        Self {
            store,
            opts,
            multistore,
            extensions,
            logger,
            mtx: Mutex::new(OperationState {
                operation: None,
                restore_snapshot: None,
                restore_chunk_index: 0,
                ch_restore: None,
                ch_restore_done: None,
            }),
        }
    }

    pub fn begin(&self, op: Operation) -> anyhow::Result<()> {
        let mut state = self.mtx.lock().unwrap();
        if state.operation.is_some() {
            anyhow::bail!("Another operation is already in progress");
        }
        state.operation = Some(op);
        Ok(())
    }

    pub fn end(&self) {
        let mut state = self.mtx.lock().unwrap();
        state.operation = None;
        state.restore_snapshot = None;
        state.restore_chunk_index = 0;
        state.ch_restore = None;
        state.ch_restore_done = None;
    }

    pub fn get_interval(&self) -> u64 {
        self.opts.interval
    }

    pub fn get_keep_recent(&self) -> u32 {
        self.opts.keep_recent
    }
}

impl Manager {
    pub async fn create_snapshot(&self, height: u64) -> anyhow::Result<Snapshot> {
        self.begin(Operation::Snapshot)?;

        let latest = self.store.get_latest_snapshot().await?;
        if let Some(latest) = latest {
            if latest.height >= height {
                anyhow::bail!("A more recent snapshot already exists at height {}", latest.height);
            }
        }

        let mut stream_writer = StreamWriter::new();
        self.multistore.snapshot(height).await?;
        for (name, extension) in &self.extensions {
            let metadata = SnapshotMetadata {
                chunk_hashes: vec![], // Fill with actual chunk hashes
            };
            stream_writer.write_metadata(name, metadata).await?;
        }

        let snapshot = self.store.save_snapshot(height, stream_writer).await?;
        self.end();
        Ok(snapshot)
    }
}

impl Manager {
    pub async fn create_snapshot(&self, height: u64) -> anyhow::Result<Snapshot> {
        self.begin(Operation::Snapshot)?;

        let latest = self.store.get_latest_snapshot().await?;
        if let Some(latest) = latest {
            if latest.height >= height {
                anyhow::bail!("A more recent snapshot already exists at height {}", latest.height);
            }
        }

        let mut stream_writer = StreamWriter::new();
        self.multistore.snapshot(height).await?;
        for (name, extension) in &self.extensions {
            let metadata = SnapshotMetadata {
                chunk_hashes: vec![], // Fill with actual chunk hashes
            };
            stream_writer.write_metadata(name, metadata).await?;
        }

        let snapshot = self.store.save_snapshot(height, stream_writer).await?;
        self.end();
        Ok(snapshot)
    }
}
