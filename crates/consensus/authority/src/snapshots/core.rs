use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{self, Receiver, Sender},
};

// Example snapshot and associated types
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub height: u64,
    pub format: u32,
    pub chunks: u32,
    pub metadata: SnapshotMetadata,
}

#[derive(Clone, Debug)]
pub struct SnapshotMetadata {
    pub chunk_hashes: Vec<Vec<u8>>,
}
pub trait Snapshotter {
    fn snapshot(&self, height: u64) -> anyhow::Result<()>;
    fn restore(&self, snapshot: &Snapshot) -> anyhow::Result<()>;
}

pub trait ExtensionSnapshotter {
    fn snapshot_name(&self) -> &str;
    fn snapshot_format(&self) -> u32;
    fn supported_formats(&self) -> &[u32];
    fn snapshot_extension(
        &self,
        height: u64,
        payload_writer: Box<dyn FnMut(Vec<u8>) -> anyhow::Result<()>>,
    ) -> anyhow::Result<()>;
    fn restore_extension(
        &self,
        height: u64,
        format: u32,
        payload_reader: Box<dyn FnMut() -> anyhow::Result<Vec<u8>>>,
    ) -> anyhow::Result<()>;
}
pub struct OperationState {
    pub operation: Option<Operation>,
    pub restore_snapshot: Option<Snapshot>,
    pub restore_chunk_index: u32,
    pub ch_restore: Option<Sender<u32>>,
    pub ch_restore_done: Option<Receiver<RestoreResult>>,
}

#[derive(Clone, Debug)]
pub struct SnapshotOptions {
    pub interval: u64,
    pub keep_recent: u32,
}

#[derive(Debug)]
pub enum Operation {
    Snapshot,
    Prune,
    Restore,
}

#[derive(Debug)]
pub struct RestoreResult {
    pub complete: bool,
    pub error: Option<anyhow::Error>,
}

pub struct StreamWriter {
    pub chunks: Vec<Vec<u8>>,
}

impl StreamWriter {
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    pub async fn write_metadata(
        &mut self,
        name: &str,
        metadata: SnapshotMetadata,
    ) -> anyhow::Result<()> {
        // Serialize and store metadata
        Ok(())
    }
}
pub struct StreamReader {
    pub chunks: Receiver<u32>,
}

impl StreamReader {
    pub fn new(chunks: Receiver<u32>) -> Self {
        Self { chunks }
    }
}
