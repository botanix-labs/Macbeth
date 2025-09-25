use log::{error, info};
use reth_fs_util::read_json_file;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

use bitcoin_hashes::Hash;
use btc_server_client::{OutPoint, UtxoToRecover};

#[derive(Debug, Error)]
pub enum UtxoRecoveryError {
    #[error("Invalid txid hex: {txid}")]
    InvalidTxid { txid: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UtxoRecoveryData {
    /// Transaction ID as hex string
    txid: String,
    /// Output index
    vout: u32,
    /// Ethereum address (empty string for change UTXOs)
    eth_address: String,
}

impl TryFrom<UtxoRecoveryData> for UtxoToRecover {
    type Error = UtxoRecoveryError;

    fn try_from(data: UtxoRecoveryData) -> Result<Self, Self::Error> {
        // Parse the hex string into a bitcoin::Txid first to ensure we handle endianness correctly
        let txid = data
            .txid
            .parse::<bitcoin::Txid>()
            .map_err(|_| UtxoRecoveryError::InvalidTxid { txid: data.txid.clone() })?;

        let bitcoin_outpoint = bitcoin::OutPoint { txid, vout: data.vout };
        let outpoint = OutPoint {
            txid: bitcoin_outpoint.txid.to_byte_array().to_vec(),
            vout: bitcoin_outpoint.vout,
        };

        Ok(UtxoToRecover { outpoint: Some(outpoint), eth_address: data.eth_address })
    }
}

/// Read UTXOs from a JSON file for recovery
pub fn read_utxos_from_file(file_path: &Path) -> Vec<UtxoToRecover> {
    match read_json_file::<Vec<UtxoRecoveryData>>(file_path) {
        Ok(utxos_data) => {
            let valid_utxos: Vec<UtxoToRecover> = utxos_data
                .into_iter()
                .filter_map(|data| match UtxoToRecover::try_from(data) {
                    Ok(utxo) => Some(utxo),
                    Err(err) => {
                        error!("utxo-recovery: Skipping invalid UTXO: {}", err);
                        None
                    }
                })
                .collect();

            info!(
                "utxo-recovery: Successfully loaded {} valid UTXOs from {:?}",
                valid_utxos.len(),
                file_path
            );
            valid_utxos
        }
        Err(err) => {
            error!("utxo-recovery: Failed to read UTXO recovery file {:?}: {}", file_path, err);
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    // first two taken from utxo_recovery.json file, last one is an example with no eth address
    fn test_read_utxos_from_file_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let json_content = r#"[
{
    "txid": "7fffc6ffc9db1400ba859447ea1f82946fa3f736f2ad1725cbd4cd1267472a1f",
    "vout": 0,
    "ethAddress": "1284fEdeda331BbD0b1a868abFeD9A3Cfb91a677"
},
{
    "txid": "d0204b10e98329ceec73bc50df687416d9c5f28d2e37fa6f1054f170ee0b4442",
    "vout": 0,
    "ethAddress": "4837f53DCD09Dca12a4761BEfAd7a2398B96617a"
},
{
    "txid": "f58feb51fbc4d7484975ced7b8649e51ba8f96d7bb00c3e49b396a080e105abf",
    "vout": 5,
    "ethAddress": ""
}
]"#;
        temp_file.write_all(json_content.as_bytes()).unwrap();

        let utxos = read_utxos_from_file(temp_file.path());

        // compare the txids. Specifically, ensure that they are being stored as little endian.
        assert_eq!(utxos.len(), 3);
        assert_eq!(
            utxos[0].outpoint.as_ref().unwrap().txid,
            hex::decode("7fffc6ffc9db1400ba859447ea1f82946fa3f736f2ad1725cbd4cd1267472a1f")
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<u8>>()
        );
        assert_eq!(utxos[0].outpoint.as_ref().unwrap().vout, 0);
        assert_eq!(utxos[0].eth_address, "1284fEdeda331BbD0b1a868abFeD9A3Cfb91a677");
        assert_eq!(
            utxos[1].outpoint.as_ref().unwrap().txid,
            hex::decode("d0204b10e98329ceec73bc50df687416d9c5f28d2e37fa6f1054f170ee0b4442")
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<u8>>()
        );
        assert_eq!(utxos[1].outpoint.as_ref().unwrap().vout, 0);
        assert_eq!(utxos[1].eth_address, "4837f53DCD09Dca12a4761BEfAd7a2398B96617a");
        assert_eq!(
            utxos[2].outpoint.as_ref().unwrap().txid,
            hex::decode("f58feb51fbc4d7484975ced7b8649e51ba8f96d7bb00c3e49b396a080e105abf")
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<u8>>()
        );
        assert_eq!(utxos[2].outpoint.as_ref().unwrap().vout, 5);
        assert_eq!(utxos[2].eth_address, "");
    }

    #[test]
    fn test_read_utxos_from_file_invalid_json() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let invalid_json = r#"[{"txid": "invalid"}"#; // Missing closing bracket
        temp_file.write_all(invalid_json.as_bytes()).unwrap();

        let utxos = read_utxos_from_file(temp_file.path());
        assert_eq!(utxos.len(), 0);
    }

    #[test]
    fn test_read_utxos_from_file_missing_file() {
        let utxos = read_utxos_from_file(Path::new("nonexistent_file.json"));
        assert_eq!(utxos.len(), 0);
    }

    #[test]
    fn test_utxo_recovery_data_conversion() {
        let data = UtxoRecoveryData {
            txid: "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_string(),
            vout: 5,
            eth_address: "0xabcdef".to_string(),
        };

        let utxo: UtxoToRecover = data.try_into().unwrap();

        assert_eq!(
            utxo.outpoint.as_ref().unwrap().txid,
            hex::decode("1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<u8>>()
        );
        assert_eq!(utxo.eth_address, "0xabcdef");
        assert_eq!(utxo.outpoint.unwrap().vout, 5);
    }
}
