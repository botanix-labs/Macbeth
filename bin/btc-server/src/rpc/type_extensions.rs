use crate::{
    pegout_scheduler::{PegoutRequest, Tx},
    rpc::{OutPoint, PendingPegout, ScriptBuf, TrackedTx, Transaction, TxIn, TxOut},
};
use bitcoin::{hashes::Hash, TxIn as BtcTxIn, TxOut as BtcTxOut, Txid};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TryFromError {
    #[error("failed to convert to prost type: ({variant})")]
    ConversionError { variant: &'static str },
}

impl TxIn {
    // validates that optional fields contain valid values
    pub fn validate(&self) -> Result<(), String> {
        // validate previous_outpoint
        if self.previous_outpoint.is_none() {
            return Err("previous_outpoint field is required".to_string());
        } else {
            let txid = self.previous_outpoint.clone().expect("outpoint to exist").txid;
            if let Err(e) = Txid::from_slice(&txid) {
                return Err(format!("invalid txid: {}", e));
            }
        }

        // validate script_sig
        if self.script_sig.is_none() {
            return Err("script_sig field is required".to_string());
        }
        Ok(())
    }
}

impl TryFrom<BtcTxIn> for TxIn {
    type Error = TryFromError;

    fn try_from(tx_in: BtcTxIn) -> Result<Self, Self::Error> {
        Ok(TxIn {
            previous_outpoint: Some(OutPoint {
                txid: tx_in.previous_output.txid.to_byte_array().to_vec(),
                vout: tx_in.previous_output.vout,
            }),
            script_sig: Some(ScriptBuf { script: tx_in.script_sig.to_bytes().to_vec() }),
            sequence: tx_in.sequence.0,
            witness: tx_in.witness.to_vec(),
        })
    }
}

impl TxOut {
    // only validates that optional fields contain values
    pub fn validate(&self) -> Result<(), String> {
        if self.script_pubkey.is_none() {
            return Err("script_pubkey field is required".to_string());
        }
        Ok(())
    }
}

impl TryFrom<BtcTxOut> for TxOut {
    type Error = TryFromError;

    fn try_from(tx_out: BtcTxOut) -> Result<Self, Self::Error> {
        Ok(TxOut {
            value: tx_out.value.to_sat(),
            script_pubkey: Some(ScriptBuf { script: tx_out.script_pubkey.to_bytes().to_vec() }),
        })
    }
}

impl TrackedTx {
    // only validates that optional fields contain values
    pub fn validate(&self) -> Result<(), String> {
        if self.tx.is_none() {
            return Err("tx field is required".to_string());
        }
        if self.created.is_none() {
            return Err("created field is required".to_string());
        }
        Ok(())
    }
}

impl TryFrom<PegoutRequest> for PendingPegout {
    type Error = TryFromError;

    fn try_from(pegout: PegoutRequest) -> Result<Self, Self::Error> {
        Ok(PendingPegout {
            pegout_id: pegout.id.as_bytes().to_vec(),
            spk: pegout.spk.into_bytes(),
            amount: pegout.value.to_sat(),
            height: pegout.botanix_height,
        })
    }
}

impl TryFrom<Tx> for TrackedTx {
    type Error = TryFromError;

    fn try_from(tx: Tx) -> Result<Self, Self::Error> {
        // create internal tx
        let tx_ins = tx
            .tx
            .input
            .into_iter()
            .map(TryFrom::try_from)
            .collect::<Result<Vec<TxIn>, _>>()
            .map_err(|_| TryFromError::ConversionError { variant: "tx_in" })?;
        let tx_outs = tx
            .tx
            .output
            .into_iter()
            .map(TryFrom::try_from)
            .collect::<Result<Vec<TxOut>, _>>()
            .map_err(|_| TryFromError::ConversionError { variant: "tx_out" })?;
        let internal_tx = Transaction {
            version: tx.tx.version.0,
            lock_time: tx.tx.lock_time.to_consensus_u32(),
            input: tx_ins,
            output: tx_outs,
        };

        // create pegout requests
        let pegout_requests = tx
            .pegout_requests
            .into_iter()
            .map(TryFrom::try_from)
            .collect::<Result<Vec<PendingPegout>, _>>()
            .map_err(|_| TryFromError::ConversionError { variant: "pending_pegout" })?;

        // create duration since epoch
        let duration = tx.created.duration_since(std::time::UNIX_EPOCH).expect("valid duration");

        // create tracked tx
        Ok(TrackedTx {
            txid: tx.txid.to_byte_array().to_vec(),
            tx: Some(internal_tx),
            pegout_idxs: tx.pegout_idxs.into_iter().map(|idx| idx as u32).collect(),
            pegout_requests,
            change_idxs: tx.change_idxs.into_iter().map(|idx| idx as u32).collect(),
            created: Some(prost_types::Timestamp {
                seconds: duration.as_secs() as i64,
                nanos: duration.subsec_nanos() as i32,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::rpc::{self, TrackedTx};
    use prost_types::Timestamp;

    #[test]
    fn test_tracked_tx_validate() {
        let tx = rpc::Transaction { version: 0, lock_time: 0, input: vec![], output: vec![] };
        let tracked_tx = TrackedTx {
            txid: vec![],
            tx: Some(tx),
            pegout_idxs: vec![],
            pegout_requests: vec![],
            change_idxs: vec![],
            created: Some(Timestamp { seconds: 0, nanos: 0 }),
        };

        assert!(tracked_tx.validate().is_ok());
    }

    #[test]
    fn test_tracked_tx_validate_missing_tx() {
        let tracked_tx = TrackedTx {
            txid: vec![],
            tx: None,
            pegout_idxs: vec![],
            pegout_requests: vec![],
            change_idxs: vec![],
            created: Some(Timestamp { seconds: 0, nanos: 0 }),
        };

        let error = tracked_tx.validate().unwrap_err();
        assert_eq!(error, "tx field is required");
    }

    #[test]
    fn test_tracked_tx_validate_missing_created() {
        let tx = rpc::Transaction { version: 0, lock_time: 0, input: vec![], output: vec![] };
        let tracked_tx = TrackedTx {
            txid: vec![],
            tx: Some(tx),
            pegout_idxs: vec![],
            pegout_requests: vec![],
            change_idxs: vec![],
            created: None,
        };

        let error = tracked_tx.validate().unwrap_err();
        assert_eq!(error, "created field is required");
    }

    #[test]
    fn test_tx_in_validate() {
        let tx_in = rpc::TxIn {
            previous_outpoint: Some(rpc::OutPoint { txid: vec![], vout: 0 }),
            script_sig: Some(rpc::ScriptBuf { script: vec![] }),
            sequence: 0,
            witness: vec![],
        };

        assert!(tx_in.validate().is_ok());
    }

    #[test]
    fn test_tx_in_validate_missing_previous_outpoint() {
        let tx_in = rpc::TxIn {
            previous_outpoint: None,
            script_sig: Some(rpc::ScriptBuf { script: vec![] }),
            sequence: 0,
            witness: vec![],
        };

        let error = tx_in.validate().unwrap_err();
        assert_eq!(error, "previous_outpoint field is required");
    }

    #[test]
    fn test_tx_in_validate_missing_script_sig() {
        let tx_in = rpc::TxIn {
            previous_outpoint: Some(rpc::OutPoint { txid: vec![], vout: 0 }),
            script_sig: None,
            sequence: 0,
            witness: vec![],
        };

        let error = tx_in.validate().unwrap_err();
        assert_eq!(error, "script_sig field is required");
    }

    #[test]
    fn test_tx_out_validate() {
        let tx_out =
            rpc::TxOut { value: 0, script_pubkey: Some(rpc::ScriptBuf { script: vec![] }) };

        assert!(tx_out.validate().is_ok());
    }

    #[test]
    fn test_tx_out_validate_missing_script_pubkey() {
        let tx_out = rpc::TxOut { value: 0, script_pubkey: None };

        let error = tx_out.validate().unwrap_err();
        assert_eq!(error, "script_pubkey field is required");
    }
}
