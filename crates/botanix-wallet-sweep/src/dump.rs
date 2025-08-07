use crate::{encoding::PARSER, request::Utxo};
use btc_server_client::{
    BtcServerExtendedApi, BtcServerExtendedClient, Empty, GetAllUtxosResponse,
};
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UtxoDump {
    utxos: Vec<Utxo>, // TODO: Define own Utxo type
}

impl UtxoDump {
    pub async fn to_bytes(&self) -> eyre::Result<Vec<u8>> {
        let encoded_data = PARSER.encode(self).await.wrap_err("Failed to encode UTXO dump")?;
        let compressed_data =
            PARSER.compress(&encoded_data).await.wrap_err("Failed to compress UTXO dump")?;
        Ok(compressed_data)
    }

    pub async fn from_bytes(bytes: &[u8]) -> eyre::Result<Self> {
        let decompressed_data =
            PARSER.decompress(bytes).await.wrap_err("Failed to decompress UTXO dump")?;

        PARSER.decode(&decompressed_data).await.wrap_err("Failed to decode UTXO dump")
    }
}

pub async fn read_dumps_from_dir(dir_path: &Path) -> eyre::Result<Vec<UtxoDump>> {
    let mut entries = tokio::fs::read_dir(dir_path).await.wrap_err("Failed to read directory")?;

    let mut utxo_dumps = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let file_path = entry.path();

        if file_path.is_file() {
            let compressed_data = tokio::fs::read(&file_path)
                .await
                .wrap_err_with(|| format!("Failed to read file: {:?}", file_path))?;

            let utxo_dump = UtxoDump::from_bytes(&compressed_data)
                .await
                .wrap_err_with(|| "Failed to decode UTXO dump")?;

            utxo_dumps.push(utxo_dump);
        }
    }

    Ok(utxo_dumps)
}

pub async fn dump_utxos(btc_server_client: &mut BtcServerExtendedClient) -> eyre::Result<UtxoDump> {
    // Read all UTXOs from the database
    let GetAllUtxosResponse { utxos: proto_utxos } = btc_server_client
        .get_all_utxos(Empty {})
        .await
        .wrap_err_with(|| "Failed to get UTXOs from BTC server")?;

    let utxos = proto_utxos.into_iter().map(TryFrom::try_from).collect::<Result<Vec<Utxo>, _>>()?;

    let utxo_dump = UtxoDump { utxos };

    Ok(utxo_dump)
}
