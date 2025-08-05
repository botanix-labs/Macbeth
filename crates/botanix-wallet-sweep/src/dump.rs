use crate::request::Utxo;
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use botanix_data_parser::DataParser;
use btc_server_client::{
    BtcServerClient, BtcServerExtendedApi, BtcServerExtendedClient, Empty, GetAllUtxosResponse,
};
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UtxoDump {
    utxos: Vec<Utxo>, // TODO: Define own Utxo type
}

pub struct UtxoDumpsReader {
    dir_path: PathBuf,
    parser: DataParser,
}

impl UtxoDumpsReader {
    pub fn new(dir_path: PathBuf, parser: DataParser) -> Self {
        Self { dir_path, parser }
    }

    pub async fn read_dumps(&self) -> eyre::Result<Vec<UtxoDump>> {
        let mut utxo_dumps = Vec::new();
        for entry in std::fs::read_dir(&self.dir_path)
            .wrap_err_with(|| format!("Failed to read directory: {:?}", self.dir_path))?
        {
            let entry = entry.wrap_err_with(|| "Failed to read directory entry")?;

            if entry.file_type().wrap_err_with(|| "Failed to get file type")?.is_file() {
                let file_path = entry.path();
                if file_path.extension().and_then(|s| s.to_str()) == Some("utxo") {
                    let compressed_data = std::fs::read(&file_path)
                        .wrap_err_with(|| format!("Failed to read file: {:?}", file_path))?;

                    let decompressed_data = self.parser.decompress(&compressed_data).await?;

                    let utxo_dump: UtxoDump = self
                        .parser
                        .decode(&decompressed_data)
                        .await
                        .wrap_err_with(|| "Failed to decode UTXO dump")?;

                    utxo_dumps.push(utxo_dump);
                }
            }
        }

        Ok(utxo_dumps)
    }
}

pub async fn dump_utxos_to_file(
    btc_server_client: &mut BtcServerExtendedClient,
    parser: &DataParser,
    output_file_path: &PathBuf,
) -> eyre::Result<()> {
    // Make sure the output file path doesn't already exist
    // and create the parent directory
    if output_file_path.exists() {
        return Err(eyre::eyre!(
            "Output file already exists: {:?}. Please specify a new file path.",
            output_file_path
        ));
    } else {
        std::fs::create_dir_all(output_file_path.parent().unwrap()).wrap_err_with(|| {
            format!("Failed to create directory for output file: {:?}", output_file_path)
        })?;
    }

    // Read all UTXOs from the database
    let GetAllUtxosResponse { utxos: proto_utxos } = btc_server_client
        .get_all_utxos(Empty {})
        .await
        .wrap_err_with(|| "Failed to get UTXOs from BTC server")?;

    let utxos = proto_utxos.into_iter().map(TryFrom::try_from).collect::<Result<Vec<Utxo>, _>>()?;

    let utxo_dump = UtxoDump { utxos };

    // TODO: Move to DumpWriter

    // Encode and compress the UTXOs
    let encoded_dump = parser.encode(&utxo_dump).await?;
    let compressed_dump = parser.compress(&encoded_dump).await?;

    // Write the compressed UTXOs to the specified output file
    std::fs::write(output_file_path, &compressed_dump)
        .wrap_err_with(|| format!("Failed to write UTXOs to file: {:?}", output_file_path))?;

    Ok(())
}
