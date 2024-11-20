use crate::rpc::{TrackedTx, TxIn, TxOut};

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

impl TxIn {
    // only validates that optional fields contain values
    pub fn validate(&self) -> Result<(), String> {
        if self.previous_outpoint.is_none() {
            return Err("previous_outpoint field is required".to_string());
        }
        if self.script_sig.is_none() {
            return Err("script_sig field is required".to_string());
        }
        Ok(())
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
