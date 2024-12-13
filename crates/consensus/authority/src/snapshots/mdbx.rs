use heed::{types::*, Database, Env, RwTxn};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Snapshot {
    pub height: u64,
    pub format: u32,
    pub chunks: u32,
    pub metadata: SnapshotMetadata,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SnapshotMetadata {
    pub chunk_hashes: Vec<Vec<u8>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExtensionMetadata {
    pub name: String,
    pub format: u32,
}

pub struct SnapshotDB {
    env: Env,
    snapshots: Database<OwnedType<u64>, SerdeBincode<Snapshot>>,
    chunks: Database<OwnedType<(u64, u32)>, ByteSlice>,
    extensions: Database<Str, SerdeBincode<ExtensionMetadata>>,
}

impl SnapshotDB {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let env = Env::new()
            .map_size(10 * 1024 * 1024) // Set map size to 10 MB
            .open(path)?;

        // Define the tables
        let snapshots = env.create_database(Some("snapshots"))?;
        let chunks = env.create_database(Some("chunks"))?;
        let extensions = env.create_database(Some("extensions"))?;

        Ok(Self { env, snapshots, chunks, extensions })
    }
}

impl SnapshotDB {
    pub fn save_snapshot(&self, snapshot: Snapshot) -> anyhow::Result<()> {
        let mut txn = self.env.write_txn()?;
        self.snapshots.put(&mut txn, &snapshot.height, &snapshot)?;
        txn.commit()?;
        Ok(())
    }
}

impl SnapshotDB {
    pub fn save_chunk(
        &self,
        height: u64,
        chunk_index: u32,
        chunk_data: &[u8],
    ) -> anyhow::Result<()> {
        let mut txn = self.env.write_txn()?;
        self.chunks.put(&mut txn, &(height, chunk_index), chunk_data)?;
        txn.commit()?;
        Ok(())
    }
}

impl SnapshotDB {
    pub fn get_snapshot(&self, height: u64) -> anyhow::Result<Option<Snapshot>> {
        let txn = self.env.read_txn()?;
        Ok(self.snapshots.get(&txn, &height)?)
    }
}

impl SnapshotDB {
    pub fn get_chunk(&self, height: u64, chunk_index: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let txn = self.env.read_txn()?;
        if let Some(data) = self.chunks.get(&txn, &(height, chunk_index))? {
            Ok(Some(data.to_vec()))
        } else {
            Ok(None)
        }
    }
}

impl SnapshotDB {
    pub fn save_extension_metadata(
        &self,
        extension_metadata: ExtensionMetadata,
    ) -> anyhow::Result<()> {
        let mut txn = self.env.write_txn()?;
        self.extensions.put(&mut txn, &extension_metadata.name, &extension_metadata)?;
        txn.commit()?;
        Ok(())
    }
}
