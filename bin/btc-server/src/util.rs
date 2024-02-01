use crate::Error;
use bitcoin::{consensus::encode as btcencode, hashes::Hash, OutPoint};
use frost_secp256k1_tr as frost;

/// Extension trait for OutPoint.
pub trait OutPointExt: Into<OutPoint> {
    fn to_bytes(self) -> [u8; 36] {
        let OutPoint { txid, vout } = self.into();
        let mut ret = [0u8; 36];
        ret[0..32].copy_from_slice(&txid[..]);
        ret[32..].copy_from_slice(&vout.to_le_bytes()[..]);
        ret
    }

    fn from_bytes(b: [u8; 36]) -> OutPoint {
        btcencode::deserialize(&b).expect("always deserializes")
    }

    fn from_slice(b: &[u8]) -> Result<OutPoint, btcencode::Error> {
        btcencode::deserialize(&b)
    }

    // stopgap for dealing with BDK with other rust-bitcoin version
    fn to_bdk(self) -> bdk::bitcoin::OutPoint {
        let OutPoint { txid, vout } = self.into();
        bdk::bitcoin::OutPoint {
            txid: bdk::bitcoin::hashes::Hash::from_slice(&txid.to_byte_array()).unwrap(),
            vout,
        }
    }

    fn from_bdk(outpoint: bdk::bitcoin::OutPoint) -> OutPoint {
        bitcoin::OutPoint { txid: outpoint.txid, vout: outpoint.vout }
    }
}

impl OutPointExt for OutPoint {}

// Deserializes a Frost peer ID.
///
/// # Arguments
///
/// * `id` - The peer ID to be decoded.
///
/// # Returns
///
/// Returns a `Result` containing the serialized Frost identifier if successful, or an `Error` if
/// the peer ID is invalid.
pub fn deserialize_frost_peer_id(id: Vec<u8>) -> Result<frost::Identifier, Error> {
    if id.len() != 32 {
        return Err(Error::InvalidFrostPeerId);
    }
    let peer_id_bytes: &[u8; 32] =
        id.as_slice().try_into().map_err(|_e| Error::InvalidFrostPeerId)?;

    let frost_id =
        frost::Identifier::deserialize(&peer_id_bytes).map_err(|_e| Error::InvalidFrostPeerId)?;

    Ok(frost_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_frost_peer_id() {
        // Valid peer ID, len = 32
        let valid_id: Vec<u8> = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C,
            0x1D, 0x1E, 0x1F, 0x20,
        ];
        let result = deserialize_frost_peer_id(valid_id);
        assert!(result.is_ok());
        result.unwrap();

        // Invalid peer ID (length is not 32)
        let invalid_id: Vec<u8> = vec![0x01, 0x02, 0x03];
        let result = deserialize_frost_peer_id(invalid_id);
        assert!(result.is_err());

        // encode and decode the id 0
        let peer_id0 = 0u16;
        let f = frost::Identifier::derive(&peer_id0.to_be_bytes().to_vec()).unwrap();
        let f_bytes = f.serialize().to_vec();
        let peer_id_decoded = deserialize_frost_peer_id(f_bytes.to_vec()).unwrap();

        assert_eq!(f, peer_id_decoded);
    }
}
